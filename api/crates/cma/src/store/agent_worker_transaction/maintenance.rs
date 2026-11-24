//! Publish dependent Agent work only under an unheld Sandbox.

use super::{Agent, ApplyError, MongoSandboxStore, SandboxStoreError, TransactionScope};
use crate::store::work_schema;
use mongodb::bson::doc;

pub(super) async fn fence_parent(
    store: &MongoSandboxStore,
    current: &Agent,
    scope: &mut TransactionScope<'_>,
) -> Result<(), ApplyError> {
    // This write and a maintenance transition contend on the same Sandbox,
    // even when the Agent claim and revision are still valid.
    let result = scope
        .update_one(
            &store.sandboxes,
            doc! {
                "_id": current.sandbox_id.as_str(),
                "document_schema_version": work_schema::versions(),
                "owner": current.owner.as_str(),
                "record.owner": current.owner.as_str(),
            },
            doc! { "$inc": { "agent_work_publication_fence": 1_i64 } },
        )
        .await
        .map_err(ApplyError::Driver)?;
    if result.matched_count == 1 {
        return Ok(());
    }

    // Preserve ordinary settlement of legacy Agents whose parent is absent.
    // A present but held or inconsistent parent never takes that path.
    let parent = scope
        .find_projected(
            &store.sandboxes,
            doc! { "_id": current.sandbox_id.as_str() },
            doc! { "_id": 1 },
        )
        .await
        .map_err(ApplyError::Driver)?;
    if parent.is_some() {
        return Err(ApplyError::Domain(SandboxStoreError::Conflict));
    }
    Ok(())
}
