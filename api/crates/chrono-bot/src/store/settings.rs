//! Deployment-wide platform settings, in MongoDB.
//!
//! There is exactly one document. Its fixed key makes that a database fact,
//! while the revision in the update filter makes every write a compare-and-set
//! rather than last-write-wins.

use layer2_core::{
    store::{PlatformSettingsStore, StoreError, StoredSettings},
    PlatformSettings,
};
use mongodb::bson::{doc, Bson, Document};
use mongodb::error::{Error, ErrorKind, WriteFailure};
use mongodb::options::ReturnDocument;
use mongodb::{Client, Collection};
use serde::{Deserialize, Serialize};

const SETTINGS: &str = "platform_settings";
const SETTINGS_ID: &str = "platform";
const DUPLICATE_KEY: i32 = 11000;

/// The singleton as it is stored.
///
/// The core deliberately owns no Mongo representation. Revisions use BSON's
/// signed integer range here and are checked on the way back into the core's
/// unsigned counter rather than silently wrapping a malformed document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct SettingsDoc {
    #[serde(rename = "_id")]
    id: String,
    settings: PlatformSettings,
    revision: i64,
    #[serde(default)]
    previous: Option<PlatformSettings>,
}

fn from_doc(doc: SettingsDoc) -> Result<StoredSettings, StoreError> {
    let revision = u64::try_from(doc.revision).map_err(|_| {
        StoreError::Malformed("platform settings revision must not be negative".into())
    })?;
    Ok(StoredSettings {
        settings: doc.settings,
        revision,
        previous: doc.previous,
    })
}

/// Build the atomic update separately so its two important properties are
/// inspectable without a database: the old object is copied to `previous`, and
/// the new object is a literal rather than an aggregation expression.
fn update_pipeline(settings: &PlatformSettings) -> Result<Vec<Document>, StoreError> {
    let settings = mongodb::bson::serialize_to_bson(settings)
        .map_err(|e| StoreError::Malformed(format!("platform settings: {e}")))?;
    Ok(vec![doc! {
        "$set": {
            "previous": { "$ifNull": ["$settings", Bson::Null] },
            // Administrator prose may legitimately begin with `$`. Without a
            // literal wrapper Mongo would read such strings as field paths.
            "settings": { "$literal": settings },
            "revision": { "$add": [
                { "$ifNull": ["$revision", 0_i64] },
                1_i64,
            ] },
        }
    }])
}

/// Whether a failed first-write upsert lost a race to the singleton key.
fn is_duplicate_key(error: &Error) -> bool {
    match error.kind.as_ref() {
        ErrorKind::Write(WriteFailure::WriteError(write)) => write.code == DUPLICATE_KEY,
        ErrorKind::Command(command) => command.code == DUPLICATE_KEY,
        _ => false,
    }
}

/// A MongoDB-backed store for the deployment's one settings document.
pub struct MongoPlatformSettingsStore {
    settings: Collection<SettingsDoc>,
}

impl MongoPlatformSettingsStore {
    /// Open the singleton over the connection shared by all Mongo stores.
    pub(super) async fn open(client: &Client, database: &str) -> Result<Self, StoreError> {
        let store = Self {
            settings: client.database(database).collection(SETTINGS),
        };
        tracing::info!(database, "platform settings store ready");
        Ok(store)
    }
}

#[async_trait::async_trait]
impl PlatformSettingsStore for MongoPlatformSettingsStore {
    async fn load(&self) -> Result<Option<StoredSettings>, StoreError> {
        self.settings
            .find_one(doc! { "_id": SETTINGS_ID })
            .await
            .map_err(|_| {
                StoreError::Unavailable("read platform settings: database read failed".into())
            })?
            .map(from_doc)
            .transpose()
    }

    async fn save_if_unchanged(
        &self,
        expected: u64,
        settings: &PlatformSettings,
    ) -> Result<Option<StoredSettings>, StoreError> {
        let Ok(expected) = i64::try_from(expected) else {
            // No stored BSON revision can equal this value.
            return Ok(None);
        };
        let pipeline = update_pipeline(settings)?;
        let result = self
            .settings
            .find_one_and_update(doc! { "_id": SETTINGS_ID, "revision": expected }, pipeline)
            .return_document(ReturnDocument::After)
            // Only revision zero describes an absent document. Upserting any
            // other expectation would manufacture the revision the caller
            // claimed to have read and incorrectly accept a stale write.
            .upsert(expected == 0)
            .await;

        match result {
            Ok(Some(doc)) => from_doc(doc).map(Some),
            Ok(None) => Ok(None),
            // Two first writers both match absence. One inserts; the other's
            // upsert collides with the fixed `_id`, which is the same semantic
            // conflict as a later stale revision.
            Err(error) if is_duplicate_key(&error) => Ok(None),
            // Do not reflect a database write error: validation diagnostics
            // can contain the submitted settings document and its prose.
            Err(_) => Err(StoreError::Unavailable(
                "write platform settings: database write failed".into(),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use layer2_core::SessionBriefing;

    fn changed() -> PlatformSettings {
        PlatformSettings {
            briefing: SessionBriefing {
                before: vec!["$an administrator may begin prose this way".into()],
                ..SessionBriefing::default()
            },
            ..PlatformSettings::default()
        }
    }

    #[test]
    fn a_stored_document_round_trips_every_setting_and_the_previous_value() {
        let document = SettingsDoc {
            id: SETTINGS_ID.into(),
            settings: changed(),
            revision: 7,
            previous: Some(PlatformSettings::default()),
        };

        let stored = from_doc(document).expect("valid document");
        assert_eq!(stored.settings, changed());
        assert_eq!(stored.revision, 7);
        assert_eq!(stored.previous, Some(PlatformSettings::default()));
    }

    #[test]
    fn a_negative_stored_revision_is_malformed_instead_of_wrapping() {
        let document = SettingsDoc {
            id: SETTINGS_ID.into(),
            settings: PlatformSettings::default(),
            revision: -1,
            previous: None,
        };

        assert!(matches!(from_doc(document), Err(StoreError::Malformed(_))));
    }

    #[test]
    fn the_atomic_update_keeps_the_old_object_and_quotes_the_new_one() {
        let pipeline = update_pipeline(&changed()).expect("serialises");
        let set = pipeline[0].get_document("$set").expect("one set stage");

        assert_eq!(
            set.get_document("previous").expect("previous expression"),
            &doc! { "$ifNull": ["$settings", Bson::Null] }
        );
        let literal = set
            .get_document("settings")
            .expect("settings expression")
            .get("$literal")
            .expect("new settings are quoted");
        let decoded: PlatformSettings = mongodb::bson::deserialize_from_bson(literal.clone())
            .expect("the quoted value is the full settings object");
        assert_eq!(decoded, changed());
    }
}
