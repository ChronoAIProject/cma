//! MongoDB persistence for deployment-wide platform settings.

use layer2_core::{PlatformSettings, PlatformSettingsStore, StoreError, StoredPlatformSettings};
use mongodb::{
    bson::{doc, Document},
    error::{Error, ErrorKind, WriteFailure},
    options::ReturnDocument,
    Client, Collection,
};
use serde::{Deserialize, Serialize};

use super::{
    durability::majority_collection,
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
}

fn from_doc(document: SettingsDoc) -> Result<StoredPlatformSettings, StoreError> {
    schema::require_current(document.document_schema_version)?;
    Ok(StoredPlatformSettings {
        revision: u64::try_from(document.revision).map_err(|_| {
            StoreError::Malformed("platform settings revision must not be negative".into())
        })?,
        settings: document.settings,
    })
}

fn update_pipeline(settings: &PlatformSettings) -> Result<Vec<Document>, StoreError> {
    let settings = mongodb::bson::serialize_to_bson(settings)
        .map_err(|error| StoreError::Malformed(format!("platform settings: {error}")))?;
    Ok(vec![doc! {
        "$set": {
            "document_schema_version": DOCUMENT_SCHEMA_VERSION,
            "settings": { "$literal": settings },
            "revision": { "$add": [{ "$ifNull": ["$revision", 0_i64] }, 1_i64] },
        }
    }])
}

/// Compare-and-set adapter for the singleton platform settings document.
pub struct MongoPlatformSettingsStore {
    settings: Collection<SettingsDoc>,
}

impl MongoPlatformSettingsStore {
    pub(super) async fn open(client: &Client, database: &str) -> Result<Self, StoreError> {
        let database = client.database(database);
        schema::initialize_collections(&database, &[SETTINGS]).await?;
        Ok(Self {
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
            .find_one(doc! { "_id": SETTINGS_ID })
            .await
            .map_err(|error| StoreError::Unavailable(format!("read platform settings: {error}")))?
            .map(from_doc)
            .transpose()
            .map(|stored| {
                stored.unwrap_or(StoredPlatformSettings {
                    revision: 0,
                    settings: PlatformSettings::default(),
                })
            })
    }

    async fn replace(
        &self,
        expected_revision: u64,
        settings: &PlatformSettings,
    ) -> Result<StoredPlatformSettings, StoreError> {
        let expected =
            i64::try_from(expected_revision).map_err(|_| StoreError::RevisionConflict {
                expected: expected_revision,
                actual: 0,
            })?;
        let result = self
            .settings
            .find_one_and_update(
                doc! {
                    "_id": SETTINGS_ID,
                    "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                    "revision": expected,
                },
                update_pipeline(settings)?,
            )
            .return_document(ReturnDocument::After)
            .upsert(expected_revision == 0)
            .await;
        match result {
            Ok(Some(document)) => from_doc(document),
            Ok(None) => Err(StoreError::RevisionConflict {
                expected: expected_revision,
                actual: self.current_revision().await?,
            }),
            Err(error) if is_duplicate_key(&error) => Err(StoreError::RevisionConflict {
                expected: expected_revision,
                actual: self.current_revision().await?,
            }),
            Err(error) => Err(StoreError::Unavailable(format!(
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

    #[test]
    fn stored_settings_validate_revision_and_quote_the_value() {
        assert!(from_doc(SettingsDoc {
            id: SETTINGS_ID.into(),
            document_schema_version: DOCUMENT_SCHEMA_VERSION,
            settings: PlatformSettings::default(),
            revision: -1,
        })
        .is_err());
        let pipeline = update_pipeline(&PlatformSettings::default()).unwrap();
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
        let pipeline = update_pipeline(&settings).unwrap();
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
