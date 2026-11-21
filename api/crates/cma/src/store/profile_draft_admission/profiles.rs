//! Apply Profile draft CAS in the transaction that reads known child fences.
use layer2_core::profiles::ProfileStoreError;
use mongodb::{
    bson::{doc, Document},
    Collection,
};
use serde::Serialize;

use super::{profile_error, transact, Error, Mutation, Pins};
use crate::store::{
    profile_documents::{draft_mutation, ProfileDocumentInput},
    sandbox_mongo::{is_duplicate_key, require_version, MongoProfileStore},
};

pub(in crate::store) async fn create(
    store: &MongoProfileStore,
    collection: &Collection<Document>,
    document: Document,
    mut pins: Pins,
) -> Result<(), ProfileStoreError> {
    let db = store.client.database(&collection.namespace().db);
    transact(
        &store.client,
        &db,
        &mut pins,
        &Mutation::Insert {
            collection: collection.clone(),
            document,
        },
    )
    .await
    .map_err(create_error)?;
    Ok(())
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
