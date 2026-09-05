//! Exact reconciliation for an ambiguous agent-work transaction commit.

use layer2_core::sandbox::store::SandboxStoreError;
use mongodb::bson::doc;

use super::agent_log::reconcile_entry;
use super::agent_worker_transaction::PublicationReceipt;
use super::agents::read_agent;
use super::sandbox_events::reconcile_event;
use super::sandbox_mongo::MongoSandboxStore;
use super::worktrees::read_worktree;

pub(super) async fn reconcile_publication(
    store: &MongoSandboxStore,
    receipt: &PublicationReceipt,
) -> Result<bool, SandboxStoreError> {
    let agent = store
        .agents
        .find_one(doc! { "_id": receipt.agent.id.as_str() })
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
        .as_ref()
        .map(read_agent)
        .transpose()?;
    if agent.as_ref() != Some(&receipt.agent) {
        return Ok(false);
    }

    if let Some(expected) = receipt.worktree.as_ref() {
        let worktree = store
            .worktrees
            .find_one(doc! { "_id": expected.id.as_str() })
            .await
            .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
            .as_ref()
            .map(read_worktree)
            .transpose()?;
        if worktree.as_ref() != Some(expected) {
            return Ok(false);
        }
    }

    for entry in &receipt.logs {
        if reconcile_entry(store, entry).await?.is_none() {
            return Ok(false);
        }
    }
    if let Some(event) = receipt.event.as_ref() {
        if reconcile_event(store, event).await?.is_none() {
            return Ok(false);
        }
    }
    Ok(true)
}
