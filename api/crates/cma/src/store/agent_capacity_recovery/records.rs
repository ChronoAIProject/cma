//! Bounded work carrier; the existing S4 Publication is completion authority.
use layer2_core::sandbox::{
    agent_capacity_recovery::state::RecoveryWork, store::SandboxStoreError,
};
use mongodb::{
    bson::{self, doc, Document},
    ClientSession, Collection,
};
use serde::{Deserialize, Serialize};

use crate::store::{
    agent_log_integrity::TransactionLogError, durability::majority_collection,
    sandbox_mongo::MongoSandboxStore,
};

pub(in crate::store) const COLLECTION: &str = "agent_capacity_recovery_work";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Row {
    #[serde(rename = "_id")]
    id: String,
    format_version: i64,
    agent_id: String,
    work: RecoveryWork,
}

pub(super) async fn read(
    store: &MongoSandboxStore,
    key: &str,
    session: &mut ClientSession,
) -> Result<Option<RecoveryWork>, TransactionLogError> {
    let row = collection(store)
        .find_one(doc! {"_id": key})
        .session(session)
        .await
        .map_err(TransactionLogError::Driver)?;
    row.map(decode).transpose().map_err(Into::into)
}

fn decode(document: Document) -> Result<RecoveryWork, SandboxStoreError> {
    if bson::serialize_to_vec(&document)
        .map_err(|_| invalid())?
        .len()
        > 32 * 1024
        || document.get_i64("format_version").ok() != Some(1)
    {
        return Err(invalid());
    }
    let row: Row = bson::deserialize_from_document(document).map_err(|_| invalid())?;
    row.work.validate().map_err(|_| invalid())?;
    if row.id != row.work.request.key().map_err(|_| invalid())?
        || row.agent_id != row.work.request.agent_id.as_str()
    {
        return Err(invalid());
    }
    Ok(row.work)
}

pub(super) fn collection(store: &MongoSandboxStore) -> Collection<Document> {
    majority_collection(
        &store.client.database(&store.agents.namespace().db),
        COLLECTION,
    )
}

pub(super) async fn insert(
    store: &MongoSandboxStore,
    work: &RecoveryWork,
    session: &mut ClientSession,
) -> Result<(), TransactionLogError> {
    work.validate().map_err(|_| invalid())?;
    let key = work.request.key().map_err(|_| invalid())?;
    let row = doc! {
        "_id": key, "format_version": 1_i64,
        "agent_id": work.request.agent_id.as_str(),
        "work": bson::serialize_to_bson(work).map_err(|_| invalid())?,
    };
    if bson::serialize_to_vec(&row).map_err(|_| invalid())?.len() > 32 * 1024 {
        return Err(invalid().into());
    }
    collection(store)
        .insert_one(row)
        .session(session)
        .await
        .map_err(|error| {
            if crate::store::sandbox_mongo::is_duplicate_key(&error) {
                TransactionLogError::Domain(SandboxStoreError::Conflict)
            } else {
                TransactionLogError::Driver(error)
            }
        })?;
    Ok(())
}

fn invalid() -> SandboxStoreError {
    SandboxStoreError::Corrupt("capacity recovery work is invalid".into())
}
