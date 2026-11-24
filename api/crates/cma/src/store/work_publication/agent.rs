//! Serialize dependent Agent writes with Sandbox maintenance.

use super::MongoSandboxStore;
use crate::store::{agent_log_integrity::TransactionLogError, work_schema};
use layer2_core::sandbox::{store::SandboxStoreError, Agent};
use mongodb::{bson::doc, ClientSession};

/// Returns false only when the parent is missing. Existing parents must match
/// the Agent's lineage and permit ordinary publication at the write.
pub(in crate::store) async fn fence_parent(
    store: &MongoSandboxStore,
    current: &Agent,
    session: &mut ClientSession,
) -> Result<bool, TransactionLogError> {
    let id = current.sandbox_id.as_str();
    let result = crate::store::work_lease_update::with_fence(
        &store.sandboxes,
        doc! {
            "_id": id,
            "document_schema_version": work_schema::versions(),
            "owner": current.owner.as_str(),
            "record.owner": current.owner.as_str(),
        },
        None,
        "agent_work_publication_fence",
        &mut *session,
    )
    .await
    .map_err(TransactionLogError::Driver)?;
    if result.matched_count == 1 {
        return Ok(true);
    }
    let parent = store
        .sandboxes
        .find_one(doc! { "_id": id })
        .projection(doc! { "_id": 1 })
        .session(session)
        .await
        .map_err(TransactionLogError::Driver)?;
    if parent.is_some() {
        return Err(SandboxStoreError::Conflict.into());
    }
    Ok(false)
}
