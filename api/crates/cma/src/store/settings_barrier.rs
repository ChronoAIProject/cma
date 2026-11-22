//! Settings writes serialize operator updates with liability in the caller's transaction.

use layer2_core::PlatformSettings;
use mongodb::{
    bson::{doc, Document},
    ClientSession, Collection,
};

use super::{schema::DOCUMENT_SCHEMA_VERSION, settings_defaults::InitialSettings};

#[derive(Debug)]
pub(super) enum Error {
    Driver(mongodb::error::Error),
    Unreadable,
}

/// Acquired anew within each attempt; never retain this across commit or retry.
#[derive(Debug)]
pub(super) struct ProtectedSettings {
    document: Document,
    settings: PlatformSettings,
}

impl ProtectedSettings {
    pub(super) fn document(&self) -> &Document {
        &self.document
    }

    pub(super) fn settings(&self) -> &PlatformSettings {
        &self.settings
    }
}

pub(super) async fn acquire(
    collection: &Collection<Document>,
    initial: InitialSettings,
    session: &mut ClientSession,
) -> Result<ProtectedSettings, Error> {
    let document = match collection
        .find_one(doc! {"_id": "platform"})
        .session(&mut *session)
        .await
        .map_err(Error::Driver)?
    {
        Some(document) => document,
        None => {
            let settings = mongodb::bson::serialize_to_bson(&initial.platform())
                .map_err(|_| Error::Unreadable)?;
            let document = doc! {
                "_id": "platform", "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                "revision": 0_i64, "settings": settings,
            };
            collection
                .insert_one(&document)
                .session(&mut *session)
                .await
                .map_err(Error::Driver)?;
            document
        }
    };
    let settings = super::settings::read_settings(document.clone())
        .map_err(|_| Error::Unreadable)?
        .settings;
    increment(collection, "admission_policy_fence", session).await?;
    Ok(ProtectedSettings { document, settings })
}

/// Also used by existing model-budget writers. Missing/invalid evidence never
/// becomes an upsert, and the field must change even when policy is unlimited.
pub(super) async fn increment(
    collection: &Collection<Document>,
    counter: &str,
    session: &mut ClientSession,
) -> Result<(), Error> {
    let result = collection
        .update_one(
            doc! {
                "_id": "platform", "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                "$or": [
                    {counter: {"$exists": false}},
                    {counter: {"$type": ["int", "long"], "$gte": 0_i64, "$lt": i64::MAX}},
                ],
            },
            doc! {"$inc": {counter: 1_i64}},
        )
        .session(session)
        .await
        .map_err(Error::Driver)?;
    if result.matched_count != 1 {
        return Err(Error::Unreadable);
    }
    Ok(())
}

#[cfg(test)]
pub(super) mod test_support;

#[cfg(test)]
mod mongo_tests;
