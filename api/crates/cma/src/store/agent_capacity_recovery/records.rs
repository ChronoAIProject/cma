//! Bounded work carrier; the existing S4 Publication is completion authority.
use layer2_core::sandbox::{
    agent_capacity_recovery::state::RecoveryWork, store::SandboxStoreError, Agent, AgentLogEntry,
};
use mongodb::{
    bson::{self, doc, Document},
    Collection,
};
use serde::{Deserialize, Serialize};

use crate::store::{
    agent_log_integrity::TransactionLogError,
    agent_log_segments::transaction_scope::TransactionScope, durability::majority_collection,
    sandbox_mongo::MongoSandboxStore,
};

pub(in crate::store) const COLLECTION: &str = "agent_capacity_recovery_work";

mod result;
mod shape;
pub(in crate::store) use result::ResultBinding;
#[cfg(test)]
pub(in crate::store) mod test_support;
#[cfg(test)]
mod tests;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::store) struct StoredRecovery {
    pub work: RecoveryWork,
    pub result: ResultBinding,
}

impl StoredRecovery {
    pub(in crate::store) fn new(
        agent: &Agent,
        work: RecoveryWork,
        logs: &[AgentLogEntry],
        now: &str,
    ) -> Result<Self, SandboxStoreError> {
        let result = ResultBinding::new(agent, &work, logs, now)?;
        Ok(Self { work, result })
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Row {
    #[serde(rename = "_id")]
    id: String,
    format_version: i64,
    agent_id: String,
    work: RecoveryWork,
    result: ResultBinding,
}

pub(in crate::store) async fn read(
    store: &MongoSandboxStore,
    key: &str,
    scope: &mut TransactionScope<'_>,
) -> Result<Option<StoredRecovery>, TransactionLogError> {
    let row = scope
        .find_one(&collection(store), doc! {"_id": key})
        .await
        .map_err(TransactionLogError::Driver)?;
    row.map(decode).transpose().map_err(Into::into)
}

fn decode(document: Document) -> Result<StoredRecovery, SandboxStoreError> {
    if bson::serialize_to_vec(&document)
        .map_err(|_| invalid())?
        .len()
        > 32 * 1024
        || document.get_i64("format_version").ok() != Some(2)
    {
        return Err(invalid());
    }
    shape::validate(&document)?;
    let row: Row = bson::deserialize_from_document(document).map_err(|_| invalid())?;
    row.work.validate().map_err(|_| invalid())?;
    row.result.validate(&row.work)?;
    if row.id != row.work.request.key().map_err(|_| invalid())?
        || row.agent_id != row.work.request.agent_id.as_str()
    {
        return Err(invalid());
    }
    Ok(StoredRecovery {
        work: row.work,
        result: row.result,
    })
}

pub(in crate::store) fn collection(store: &MongoSandboxStore) -> Collection<Document> {
    majority_collection(
        &store.client.database(&store.agents.namespace().db),
        COLLECTION,
    )
}

pub(in crate::store) async fn insert(
    store: &MongoSandboxStore,
    work: &StoredRecovery,
    scope: &mut TransactionScope<'_>,
) -> Result<(), TransactionLogError> {
    let row = encode(work)?;
    scope
        .insert_one(&collection(store), row)
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

fn encode(stored: &StoredRecovery) -> Result<Document, SandboxStoreError> {
    let work = &stored.work;
    stored.result.validate(work)?;
    let key = work.request.key().map_err(|_| invalid())?;
    let row = doc! {
        "_id": key, "format_version": 2_i64,
        "agent_id": work.request.agent_id.as_str(),
        "work": bson::serialize_to_bson(work).map_err(|_| invalid())?,
        "result": bson::serialize_to_bson(&stored.result).map_err(|_| invalid())?,
    };
    shape::validate(&row)?;
    if bson::serialize_to_vec(&row).map_err(|_| invalid())?.len() > 32 * 1024 {
        return Err(invalid());
    }
    Ok(row)
}

fn invalid() -> SandboxStoreError {
    SandboxStoreError::Corrupt("capacity recovery work is invalid".into())
}

/// Replace only the exact prior carrier, in the same transaction as S4's sole
/// Agent CAS/publication. A successful carrier update alone grants no authority.
pub(in crate::store) async fn replace(
    store: &MongoSandboxStore,
    current: &StoredRecovery,
    next: &StoredRecovery,
    scope: &mut TransactionScope<'_>,
) -> Result<(), TransactionLogError> {
    let filter = encode(current)?;
    // Classify changes to immutable identity or the expected revision as CAS
    // conflicts before validating the proposed row's dependent fields. For
    // example, changing requested_at can also invalidate ready_at; it must
    // never bypass the original-request conflict boundary.
    if current.work.request != next.work.request
        || current.work.target != next.work.target
        || current.work.revision.checked_add(1) != Some(next.work.revision)
        || current.result.agent_revision >= next.result.agent_revision
        || current.result.through_seq > next.result.through_seq
    {
        return Err(SandboxStoreError::Conflict.into());
    }
    let mut encoded = encode(next)?;
    let next = encoded.remove("work").ok_or_else(invalid)?;
    let result = encoded.remove("result").ok_or_else(invalid)?;
    let updated = scope
        .update_one(
            &collection(store),
            filter,
            doc! {"$set": {"work": next, "result": result}},
        )
        .await
        .map_err(TransactionLogError::Driver)?;
    if updated.matched_count != 1 {
        return Err(SandboxStoreError::Conflict.into());
    }
    Ok(())
}
