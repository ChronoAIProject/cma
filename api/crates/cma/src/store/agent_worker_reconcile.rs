//! Exact reconciliation for an ambiguous agent-work transaction commit.

use crate::store::agent_log_telemetry::ScanCaller;
use layer2_core::sandbox::store::SandboxStoreError;
use mongodb::bson::doc;
use mongodb::options::ReadConcern;

use super::agent_log::scan_complete_log_with_snapshot_session;
use super::agent_log_integrity::TransactionLogError;
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
    if !receipt.pending_adoption() && agent.as_ref() != Some(&receipt.agent) {
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

    if let Some(expected) = receipt
        .worktree
        .as_ref()
        .filter(|_| !receipt.pending_adoption())
    {
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
    if let Some(recorded) = &receipt.segmented {
        if recorded.next != receipt.agent
            || recorded.logs != receipt.logs
            || !recorded.matches(store, session).await?
        {
            return Ok(false);
        }
    } else {
        let first_receipt_seq = receipt.logs.first().map(|entry| entry.seq);
        let mut receipt_offset = 0;
        scan_complete_log_with_snapshot_session(
            ScanCaller::UnknownCommitProof,
            store,
            &receipt.agent.id,
            receipt.agent.conversation.last_seq,
            session,
            |entry| {
                if first_receipt_seq.is_some_and(|first| entry.seq >= first) {
                    let Some(expected) = receipt.logs.get(receipt_offset) else {
                        return Err(SandboxStoreError::SequenceConflict);
                    };
                    if &entry != expected {
                        return Err(SandboxStoreError::SequenceConflict);
                    }
                    receipt_offset += 1;
                }
                Ok(())
            },
        )
        .await
        .map_err(log_error)?;
        if receipt_offset != receipt.logs.len() {
            return Ok(false);
        }
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

fn log_error(error: TransactionLogError) -> SandboxStoreError {
    match error {
        TransactionLogError::Domain(error) => error,
        TransactionLogError::Driver(error) => SandboxStoreError::Unavailable(error.to_string()),
    }
}
