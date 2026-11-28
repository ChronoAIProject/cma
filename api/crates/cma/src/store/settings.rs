//! MongoDB persistence for deployment-wide platform settings.

use layer2_core::{
    PlatformSettingChanges, PlatformSettings, PlatformSettingsStore, StoreError,
    StoredPlatformSettings,
};
use mongodb::{
    bson::{doc, Bson, Document},
    error::{Error, ErrorKind, WriteFailure},
    Client, Collection,
};
use serde::{Deserialize, Serialize};

use super::{
    durability::majority_collection,
    profile_draft_admission::{self, Error as AdmissionError, Mutation, Pins},
    schema::{self, DOCUMENT_SCHEMA_VERSION},
};

pub(super) const SETTINGS: &str = "platform_settings";
const SETTINGS_ID: &str = "platform";
const DUPLICATE_KEY: i32 = 11_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct SettingsDoc {
    #[serde(rename = "_id")]
    id: String,
    document_schema_version: i64,
    settings: PlatformSettings,
    revision: i64,
    #[serde(default)]
    setting_changes: Option<Bson>,
}

#[derive(Serialize, Deserialize)]
struct SettingChanges {
    coverage_revision: u64,
    entries: PlatformSettingChanges,
}

pub(super) fn read_settings(document: Document) -> Result<StoredPlatformSettings, StoreError> {
    let document = mongodb::bson::deserialize_from_document(document)
        .map_err(|error| StoreError::Malformed(format!("platform settings: {error}")))?;
    let stored = from_doc(document)?;
    stored
        .settings
        .validate()
        .map_err(|error| StoreError::Malformed(error.to_string()))?;
    Ok(stored)
}

fn from_doc(document: SettingsDoc) -> Result<StoredPlatformSettings, StoreError> {
    schema::require_current(document.document_schema_version)?;
    let revision = u64::try_from(document.revision).map_err(|_| {
        StoreError::Malformed("platform settings revision must not be negative".into())
    })?;
    let changes = match document.setting_changes {
        Some(Bson::Document(metadata)) => {
            mongodb::bson::deserialize_from_document::<SettingChanges>(metadata)
                .ok()
                .filter(|metadata| metadata.coverage_revision == revision)
                .map(|metadata| metadata.entries)
                .unwrap_or_default()
        }
        _ => Default::default(),
    };
    Ok(StoredPlatformSettings {
        revision,
        settings: document.settings,
        changes,
    })
}

fn update_pipeline(
    settings: &PlatformSettings,
    changes: Option<(u64, &PlatformSettingChanges)>,
) -> Result<Vec<Document>, StoreError> {
    let settings = mongodb::bson::serialize_to_bson(settings)
        .map_err(|error| StoreError::Malformed(format!("platform settings: {error}")))?;
    let mut set = doc! {
        "document_schema_version": DOCUMENT_SCHEMA_VERSION,
        "settings": { "$literal": settings },
        "revision": { "$add": [{ "$ifNull": ["$revision", 0_i64] }, 1_i64] },
    };
    if let Some((revision, entries)) = changes {
        let metadata = mongodb::bson::serialize_to_bson(&SettingChanges {
            coverage_revision: revision,
            entries: entries.clone(),
        })
        .map_err(|error| StoreError::Malformed(format!("platform setting changes: {error}")))?;
        set.insert("setting_changes", doc! { "$literal": metadata });
    }
    Ok(vec![doc! { "$set": set }])
}

/// Compare-and-set adapter for the singleton platform settings document.
pub struct MongoPlatformSettingsStore {
    initial_settings: super::settings_defaults::InitialSettings,
    client: Client,
    settings: Collection<SettingsDoc>,
}

impl MongoPlatformSettingsStore {
    pub(crate) async fn read_for_startup(
        &self,
    ) -> Result<Option<StoredPlatformSettings>, StoreError> {
        match self.read().await {
            Ok(settings) => Ok(Some(settings)),
            Err(StoreError::Malformed(_)) => {
                tracing::error!(
                    collection = SETTINGS,
                    record = SETTINGS_ID,
                    "Platform settings quarantined"
                );
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }

    #[cfg(test)]
    pub(super) async fn open(client: &Client, database: &str) -> Result<Self, StoreError> {
        Self::open_with_defaults(client, database, Default::default()).await
    }

    pub(super) async fn open_with_defaults(
        client: &Client,
        database: &str,
        initial_settings: super::settings_defaults::InitialSettings,
    ) -> Result<Self, StoreError> {
        let database = client.database(database);
        schema::initialize_collections(&database, &[SETTINGS]).await?;
        Ok(Self {
            initial_settings,
            client: client.clone(),
            settings: majority_collection(&database, SETTINGS),
        })
    }

    async fn current_revision(&self) -> Result<u64, StoreError> {
        Ok(self
            .settings
            .find_one(doc! { "_id": SETTINGS_ID })
            .await
            .map_err(|error| {
                StoreError::Unavailable(format!("read platform settings revision: {error}"))
            })?
            .map(from_doc)
            .transpose()?
            .map_or(0, |stored| stored.revision))
    }
}

#[async_trait::async_trait]
impl PlatformSettingsStore for MongoPlatformSettingsStore {
    async fn read(&self) -> Result<StoredPlatformSettings, StoreError> {
        self.settings
            .clone_with_type::<Document>()
            .find_one(doc! { "_id": SETTINGS_ID })
            .await
            .map_err(|error| StoreError::Unavailable(format!("read platform settings: {error}")))?
            .map(read_settings)
            .transpose()
            .map(|stored| {
                stored.unwrap_or(StoredPlatformSettings {
                    revision: 0,
                    changes: Default::default(),
                    settings: self.initial_settings.platform(),
                })
            })
    }

    async fn replace(
        &self,
        expected_revision: u64,
        settings: &PlatformSettings,
    ) -> Result<StoredPlatformSettings, StoreError> {
        self.replace_settings(expected_revision, settings, None)
            .await
    }

    async fn replace_with_changes(
        &self,
        expected_revision: u64,
        settings: &PlatformSettings,
        changes: &PlatformSettingChanges,
    ) -> Result<StoredPlatformSettings, StoreError> {
        self.replace_settings(expected_revision, settings, Some(changes))
            .await
    }
}

impl MongoPlatformSettingsStore {
    async fn replace_settings(
        &self,
        expected_revision: u64,
        settings: &PlatformSettings,
        changes: Option<&PlatformSettingChanges>,
    ) -> Result<StoredPlatformSettings, StoreError> {
        let expected =
            i64::try_from(expected_revision).map_err(|_| StoreError::RevisionConflict {
                expected: expected_revision,
                actual: 0,
            })?;
        let database = self.client.database(&self.settings.namespace().db);
        let mut pins = Pins::defaults(&settings.defaults);
        let result = profile_draft_admission::transact(
            &self.client,
            &database,
            &mut pins,
            &Mutation::Settings {
                collection: self.settings.clone_with_type::<Document>(),
                filter: doc! {
                    "_id": SETTINGS_ID,
                    "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                    "revision": expected,
                },
                pipeline: update_pipeline(
                    settings,
                    changes.map(|entries| (expected_revision + 1, entries)),
                )?,
                upsert: expected_revision == 0,
            },
        )
        .await;
        match result {
            Ok(Some(document)) => from_doc(
                mongodb::bson::deserialize_from_document(document).map_err(|error| {
                    StoreError::Malformed(format!("platform settings: {error}"))
                })?,
            ),
            Ok(None) => Err(StoreError::RevisionConflict {
                expected: expected_revision,
                actual: self.current_revision().await?,
            }),
            Err(AdmissionError::Driver(error))
                if is_duplicate_key(&error)
                    || profile_draft_admission::is_transaction_conflict(&error) =>
            {
                Err(StoreError::RevisionConflict {
                    expected: expected_revision,
                    actual: self.current_revision().await?,
                })
            }
            Err(AdmissionError::ReferenceUnavailable) => Err(StoreError::ReferenceUnavailable(
                "platform defaults reference an unavailable Profile".into(),
            )),
            Err(AdmissionError::Driver(error)) => Err(StoreError::Unavailable(format!(
                "replace platform settings: {error}"
            ))),
        }
    }
}

fn is_duplicate_key(error: &Error) -> bool {
    match error.kind.as_ref() {
        ErrorKind::Write(WriteFailure::WriteError(write)) => write.code == DUPLICATE_KEY,
        ErrorKind::Command(command) => command.code == DUPLICATE_KEY,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
    async fn settings_quarantine_preserves_the_record_and_refuses_policy_reads() {
        use crate::auth::registration::RegistrationCapacity;

        let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
        let client = Client::with_uri_str(uri).await.unwrap();
        let name = format!("settings_quarantine_{}", uuid::Uuid::new_v4().simple());
        let database = client.database(&name);
        let store = MongoPlatformSettingsStore::open(&client, &name)
            .await
            .unwrap();
        let rows = database.collection::<Document>(SETTINGS);
        let broken = doc! {
            "_id": SETTINGS_ID, "revision": 1_i64,
            "document_schema_version": DOCUMENT_SCHEMA_VERSION, "settings": "invalid",
        };
        rows.insert_one(&broken).await.unwrap();
        let registrations =
            crate::store::MongoRegistrationCapacityStore::open(&client, &name, true)
                .await
                .unwrap();
        assert!(!registrations.available().await.unwrap());
        assert!(!registrations
            .admit(&layer2_core::SubjectId::new("new-subject"))
            .await
            .unwrap());
        for _ in 0..2 {
            assert_eq!(store.read_for_startup().await.unwrap(), None);
            assert!(matches!(store.read().await, Err(StoreError::Malformed(_))));
            assert_eq!(
                rows.find_one(doc! {"_id": SETTINGS_ID}).await.unwrap(),
                Some(broken.clone())
            );
        }
        database.drop().await.unwrap();
    }

    #[test]
    fn previous_writer_advancing_revision_masks_last_change() {
        let settings = PlatformSettings::default();
        let entries = PlatformSettingChanges::from([(
            "agent_log_retention".into(),
            layer2_core::PlatformSettingChange {
                actor: layer2_core::SubjectId::new("operator-test"),
                at: "2026-01-02T03:04:05Z".into(),
                revision: 1,
            },
        )]);
        let metadata = mongodb::bson::serialize_to_bson(&SettingChanges {
            coverage_revision: 1,
            entries: entries.clone(),
        })
        .unwrap();
        let current = SettingsDoc {
            id: SETTINGS_ID.into(),
            document_schema_version: DOCUMENT_SCHEMA_VERSION,
            settings,
            revision: 1,
            setting_changes: Some(metadata),
        };
        assert_eq!(from_doc(current.clone()).unwrap().changes, entries);
        let previous_writer = SettingsDoc {
            revision: 2,
            ..current
        };
        assert!(from_doc(previous_writer).unwrap().changes.is_empty());
    }

    #[test]
    fn stored_settings_validate_revision_and_quote_the_value() {
        assert!(from_doc(SettingsDoc {
            id: SETTINGS_ID.into(),
            document_schema_version: DOCUMENT_SCHEMA_VERSION,
            settings: PlatformSettings::default(),
            revision: -1,
            setting_changes: None,
        })
        .is_err());
        let pipeline = update_pipeline(&PlatformSettings::default(), None).unwrap();
        let set = pipeline[0].get_document("$set").unwrap();
        assert_eq!(
            set.get_i64("document_schema_version").unwrap(),
            DOCUMENT_SCHEMA_VERSION
        );
        assert!(set
            .get_document("settings")
            .unwrap()
            .contains_key("$literal"));
    }

    #[test]
    fn disabled_settings_pipeline_keeps_the_predecessor_inactivity_shape() {
        let mut settings = PlatformSettings::default();
        settings.inactivity.pause_after_seconds = 1_200;
        let pipeline = update_pipeline(&settings, None).unwrap();
        let literal = pipeline[0]
            .get_document("$set")
            .unwrap()
            .get_document("settings")
            .unwrap()
            .get_document("$literal")
            .unwrap();
        assert_eq!(
            literal.get_document("inactivity").unwrap(),
            &doc! { "pause_after_seconds": 1_200_i64 }
        );
        assert_eq!(
            mongodb::bson::deserialize_from_document::<PlatformSettings>(literal.clone()).unwrap(),
            settings
        );
    }
}
