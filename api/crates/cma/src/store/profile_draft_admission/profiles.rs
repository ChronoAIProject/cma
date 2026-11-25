//! Apply Profile draft CAS in the transaction that reads known child fences.
use layer2_core::profiles::ProfileStoreError;
use mongodb::{
    bson::{doc, Bson, Document},
    error::UNKNOWN_TRANSACTION_COMMIT_RESULT,
    Collection,
};
use serde::Serialize;

use super::{profile_error, transact, transact_in, Error, Mutation, Pins};
use crate::store::{
    profile_documents::{draft_mutation, ProfileDocumentInput},
    profiles::read_after_unknown_commit,
    sandbox_mongo::{is_duplicate_key, require_version, MongoProfileStore},
};

pub(in crate::store) async fn create(
    store: &MongoProfileStore,
    collection: &Collection<Document>,
    document: Document,
    mut pins: Pins,
) -> Result<(), ProfileStoreError> {
    let db = store.client.database(&collection.namespace().db);
    let mutation = Mutation::Insert {
        collection: collection.clone(),
        document: document.clone(),
    };
    let mut session = store.client.start_session().await.map_err(unavailable)?;
    // A failed create never replays its mutation. Only an unknown commit is
    // reconciled, and only the exact durable draft proves that it applied.
    match transact_in(&mut session, &db, &mut pins, &mutation).await {
        Ok(_) => Ok(()),
        Err(Error::Driver(error)) if error.contains_label(UNKNOWN_TRANSACTION_COMMIT_RESULT) => {
            let id = document.get("_id").cloned().unwrap_or(Bson::Null);
            match read_after_unknown_commit(&mut session, collection, doc! {"_id": id}).await? {
                Some(stored) if stored == document => Ok(()),
                _ => Err(create_error(Error::Driver(error))),
            }
        }
        Err(error) => Err(create_error(error)),
    }
}

fn unavailable(error: mongodb::error::Error) -> ProfileStoreError {
    ProfileStoreError::Unavailable(error.to_string())
}

fn create_error(error: Error) -> ProfileStoreError {
    match error {
        Error::ReferenceUnavailable => ProfileStoreError::ReferenceUnavailable,
        Error::Driver(error) if is_duplicate_key(&error) => ProfileStoreError::Duplicate,
        Error::Driver(error) => ProfileStoreError::Unavailable(error.to_string()),
    }
}

pub(in crate::store) async fn save<T: Serialize>(
    store: &MongoProfileStore,
    collection: &Collection<Document>,
    expected: u64,
    input: ProfileDocumentInput<'_, T>,
    mut pins: Pins,
) -> Result<(), ProfileStoreError> {
    let (filter, update) = draft_mutation(expected, &input)?;
    let db = store.client.database(&collection.namespace().db);
    if transact(
        &store.client,
        &db,
        &mut pins,
        &Mutation::Update {
            collection: collection.clone(),
            filter,
            update,
        },
    )
    .await
    .map_err(profile_error)?
    .is_some()
    {
        return Ok(());
    }
    let current = collection
        .find_one(doc! {"_id":input.id})
        .projection(doc! {"document_schema_version":1})
        .await
        .map_err(|error| ProfileStoreError::Unavailable(error.to_string()))?;
    if let Some(current) = current {
        require_version(&current).map_err(ProfileStoreError::Corrupt)?;
        Err(ProfileStoreError::Conflict)
    } else {
        Err(ProfileStoreError::NotFound)
    }
}
