//! Exact reconciliation for an ambiguous agent-work transaction commit.

use layer2_core::sandbox::store::SandboxStoreError;
use mongodb::bson::doc;
use mongodb::options::ReadConcern;

use super::agent_worker_transaction::PublicationReceipt;
use super::agents::read_agent;
use super::sandbox_events::reconcile_event_with_snapshot_session;
use super::sandbox_mongo::MongoSandboxStore;
use super::sandboxes::read_sandbox;
use super::worktrees::read_worktree;

pub(super) async fn reconcile_publication_after_unknown(
    store: &MongoSandboxStore,
    receipt: &PublicationReceipt,
    snapshot_time: mongodb::bson::Timestamp,
) -> Result<bool, SandboxStoreError> {
    let mut session = store
        .client
        .start_session()
        .snapshot(true)
        .snapshot_time(snapshot_time)
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
    reconcile_publication_snapshot(store, receipt, &mut session).await
}

#[cfg(test)]
pub(super) async fn reconcile_publication(
    store: &MongoSandboxStore,
    receipt: &PublicationReceipt,
) -> Result<bool, SandboxStoreError> {
    let mut session = store
        .client
        .start_session()
        .snapshot(true)
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
    reconcile_publication_snapshot(store, receipt, &mut session).await
}

async fn reconcile_publication_snapshot(
    store: &MongoSandboxStore,
    receipt: &PublicationReceipt,
    session: &mut mongodb::ClientSession,
) -> Result<bool, SandboxStoreError> {
    let digest = super::agent_log_segments::publication::result_digest(&(
        &receipt.agent,
        &receipt.worktree,
        &receipt.logs,
        &receipt.event,
        &receipt.sign_in_sandbox,
    ))?;
    if let Some(committed) = super::agent_log_segments::publication::reconcile(
        store,
        receipt.publication.as_ref(),
        &receipt.agent,
        &receipt.logs,
        &digest,
        session,
    )
    .await?
    {
        if !committed {
            return Ok(false);
        }
        return match &receipt.event {
            Some(event) => Ok(reconcile_event_with_snapshot_session(store, event, session)
                .await?
                .is_some()),
            None => Ok(true),
        };
    }
    let agent = store
        .agents
        .find_one(doc! { "_id": receipt.agent.id.as_str() })
        .read_concern(ReadConcern::snapshot())
        .session(&mut *session)
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
        .as_ref()
        .map(read_agent)
        .transpose()?;
    if agent.as_ref() != Some(&receipt.agent) {
        return Ok(false);
    }

    if let Some(expected) = receipt.sign_in_sandbox.as_ref() {
        let sandbox = store
            .sandboxes
            .find_one(doc! { "_id": expected.id.as_str() })
            .read_concern(ReadConcern::snapshot())
            .session(&mut *session)
            .await
            .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
            .as_ref()
            .map(read_sandbox)
            .transpose()?;
        if sandbox.as_ref() != Some(expected) {
            return Ok(false);
        }
    }

    if let Some(expected) = receipt.worktree.as_ref() {
        let worktree = store
            .worktrees
            .find_one(doc! { "_id": expected.id.as_str() })
            .read_concern(ReadConcern::snapshot())
            .session(&mut *session)
            .await
            .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
            .as_ref()
            .map(read_worktree)
            .transpose()?;
        if worktree.as_ref() != Some(expected) {
            return Ok(false);
        }
    }

    if !receipt.logs.is_empty()
        && receipt.logs.last().map(|entry| entry.seq) != Some(receipt.agent.conversation.last_seq)
    {
        return Ok(false);
    }
    if !super::agent_log_receipts::matches(store, &receipt.agent, &receipt.logs, session).await? {
        return Ok(false);
    }
    if let Some(event) = receipt.event.as_ref() {
        if reconcile_event_with_snapshot_session(store, event, session)
            .await?
            .is_none()
        {
            return Ok(false);
        }
    }
    Ok(true)
}
