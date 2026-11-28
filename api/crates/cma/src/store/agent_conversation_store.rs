//! Atomic Mongo persistence for runtime conversation updates.

use crate::store::agent_log_telemetry::ScanCaller;
use crate::store::transaction_terminal::GuardedTerminal as _;
use layer2_core::ids::AgentId;
use layer2_core::sandbox::store::{
    AgentConversationMutation, AgentConversationStore, SandboxStoreError,
};
use layer2_core::sandbox::{Agent, AgentLogEntry, PendingAgentApprovals};
use layer2_core::seconds_from_rfc3339;
use mongodb::bson::doc;
use mongodb::options::ReadConcern;

use super::agent_log::{scan_complete_log_with_session, scan_complete_log_with_snapshot_session};
use super::agent_log_integrity::{
    ensure_append_batch_size, prepare_append, trusted_summary, AppendCapacity, TransactionLogError,
};
use super::agents::{agent_replacement_fields, read_agent};
use super::contention;
use super::sandbox_mongo::{
    commit_transaction, is_duplicate_key, is_transient, signed, wait_for_reconciliation,
    CommitOutcome, MongoSandboxStore, DOCUMENT_SCHEMA_VERSION, TRANSACTION_ATTEMPTS,
};

#[cfg(test)]
use super::sandbox_mongo::start_transaction;

mod admission;
#[cfg(test)]
mod admission_tests;
pub(in crate::store) mod capacity_prefix;
pub(in crate::store) mod checkpoint;
#[cfg(test)]
mod checkpoint_tests;
mod cleanup_admission;
mod implementation;
pub(in crate::store) mod publication;
use super::agent_log_segments::{conversation as scoped, transaction_scope::TransactionScope};
#[cfg(test)]
use publication::apply_mutation;
pub(in crate::store) mod reopen;
#[cfg(test)]
mod reopen_tests;

pub(super) use checkpoint::carry_fields as carry_checkpoint_fields;

fn publish_work(store: &MongoSandboxStore, logs: &[AgentLogEntry], confirmation: &'static str) {
    crate::telemetry::work::published(logs, confirmation);
    if let Some(terminal) = logs.iter().find(|entry| {
        matches!(
            entry.kind,
            layer2_core::sandbox::AgentLogKind::TurnCompleted { .. }
                | layer2_core::sandbox::AgentLogKind::TurnFailed { .. }
                | layer2_core::sandbox::AgentLogKind::TurnInterrupted { .. }
        )
    }) {
        let _ = store.agent_turn_terminal.send(terminal.agent_id.clone());
    }
}

#[cfg_attr(test, derive(Clone))]
pub(super) struct ConversationReceipt {
    pub(super) segmented: Option<super::agent_log_segments::writer_commit::Recorded>,
    pub(super) command_dispatch: Option<layer2_core::sandbox::agent_io::CommandDispatch>,
    pub(super) agent: Agent,
    pub(super) logs: Vec<AgentLogEntry>,
}

#[derive(Debug)]
pub(super) enum ApplyError {
    Domain(SandboxStoreError),
    Driver(mongodb::error::Error),
}

impl From<TransactionLogError> for ApplyError {
    fn from(error: TransactionLogError) -> Self {
        match error {
            TransactionLogError::Domain(error) => Self::Domain(error),
            TransactionLogError::Driver(error) => Self::Driver(error),
        }
    }
}

fn commit_conversation(
    store: &MongoSandboxStore,
    mutation: AgentConversationMutation,
) -> impl std::future::Future<Output = Result<Vec<AgentLogEntry>, SandboxStoreError>> + '_ {
    Box::pin(commit_conversation_inner(store, mutation))
}

async fn commit_conversation_inner(
    store: &MongoSandboxStore,
    mutation: AgentConversationMutation,
) -> Result<Vec<AgentLogEntry>, SandboxStoreError> {
    ensure_append_batch_size(mutation.agent_logs.len())?;
    store.prepare_fleet_log(&mutation.agent.id).await;
    let _writer = store.agent_writes.lock(&mutation.agent.id).await;
    let _platform = store.conversation_platform_guard(&mutation).await;
    let adoption = Box::pin(scoped::adoption::prepare(store, &mutation, None)).await?;
    let mut last_transient = None;
    for attempt in 0..TRANSACTION_ATTEMPTS {
        contention::backoff(attempt).await;
        let mut session = match store.client.start_session().await {
            Ok(session) => session,
            Err(error) if is_transient(&error) => {
                last_transient = Some(error.to_string());
                continue;
            }
            Err(error) => return Err(SandboxStoreError::Unavailable(error.to_string())),
        };
        let mut scope = match TransactionScope::start(&mut session).await {
            Ok(scope) => scope,
            Err(error) if is_transient(&error) => {
                last_transient = Some(error.to_string());
                continue;
            }
            Err(error) => return Err(SandboxStoreError::Unavailable(error.to_string())),
        };
        let applied = Box::pin(scoped::apply(
            scoped::PublicationSource::Conversation,
            store,
            &mutation,
            None,
            None,
            adoption.as_ref(),
            &mut scope,
        ))
        .await;
        scope.release();
        let receipt = match applied {
            Ok(receipt) => receipt,
            Err(ApplyError::Domain(error)) => {
                let _ = session.abort_guarded().await;
                return Err(error);
            }
            Err(ApplyError::Driver(error)) if is_transient(&error) => {
                let _ = session.abort_guarded().await;
                last_transient = Some(error.to_string());
                continue;
            }
            Err(ApplyError::Driver(error)) => {
                let _ = session.abort_guarded().await;
                return Err(SandboxStoreError::Unavailable(error.to_string()));
            }
        };
        admission::require_current(&mutation, &mut session).await?;
        match commit_transaction(&mut session).await {
            CommitOutcome::Committed => {
                publish_work(store, &receipt.logs, "committed");
                return Ok(receipt.logs);
            }
            CommitOutcome::Retry(error) => last_transient = Some(error.to_string()),
            CommitOutcome::Unknown(error) => {
                let snapshot_time = session.operation_time().ok_or_else(|| {
                    SandboxStoreError::Unavailable(format!(
                        "agent conversation commit remained unknown without an operation time: {error}"
                    ))
                })?;
                let commit_error = error.to_string();
                let unresolved_error = commit_error.clone();
                let store = store.clone();
                return wait_for_reconciliation("agent conversation commit", async move {
                    if Box::pin(reconcile_conversation_after_commit(
                        &store,
                        &receipt,
                        snapshot_time,
                    ))
                    .await?
                    {
                        publish_work(&store, &receipt.logs, "reconciled");
                        Ok(receipt.logs)
                    } else {
                        Err(SandboxStoreError::Unavailable(format!(
                            "agent conversation commit remained unknown: {unresolved_error}"
                        )))
                    }
                })
                .await
                .map_err(|deadline| {
                    SandboxStoreError::Unavailable(format!(
                        "{deadline}; ambiguous commit: {commit_error}"
                    ))
                })?;
            }
            CommitOutcome::Failed(error) => {
                return Err(SandboxStoreError::Unavailable(error.to_string()));
            }
        }
    }
    Err(contention::exhausted(last_transient))
}

pub(super) fn validate_mutation(
    current: &Agent,
    actual_last_seq: u64,
    mutation: &AgentConversationMutation,
) -> Result<(), SandboxStoreError> {
    let count = u64::try_from(mutation.agent_logs.len())
        .map_err(|_| SandboxStoreError::SequenceConflict)?;
    let next_revision = mutation
        .expected_agent_revision
        .checked_add(1)
        .ok_or(SandboxStoreError::Conflict)?;
    let next_last_seq = mutation
        .expected_last_seq
        .checked_add(count)
        .ok_or(SandboxStoreError::SequenceConflict)?;

    if seconds_from_rfc3339(&mutation.appended_at).is_none()
        || mutation.runtime_generation == 0
        || mutation.agent_logs.is_empty()
        || !current.status.is_live()
        || !mutation.agent.status.is_live()
        || current.revision != mutation.expected_agent_revision
        || mutation.agent.revision != next_revision
        || current.conversation.last_seq != mutation.expected_last_seq
        || actual_last_seq != mutation.expected_last_seq
        || mutation.agent.conversation.last_seq != next_last_seq
        || current.conversation.conversation_id != mutation.agent.conversation.conversation_id
        || current.conversation.runtime_generation != Some(mutation.runtime_generation)
        || mutation.agent.conversation.runtime_generation != Some(mutation.runtime_generation)
        || immutable_fields_changed(current, &mutation.agent)
        || mutation.agent.validate_conversation().is_err()
    {
        return Err(SandboxStoreError::Conflict);
    }
    Ok(())
}

fn immutable_fields_changed(current: &Agent, next: &Agent) -> bool {
    current.id != next.id
        || current.worktree_id != next.worktree_id
        || current.workspace_id != next.workspace_id
        || current.sandbox_id != next.sandbox_id
        || current.owner != next.owner
        || current.creation_key != next.creation_key
        || current.creation_fingerprint != next.creation_fingerprint
        || current.profile != next.profile
        || current.capabilities != next.capabilities
        || (current.work != next.work
            && !layer2_core::sandbox::agent_live_skills::is_install_admission(current, next))
        || current.failure != next.failure
        || current.title != next.title
        || current.created_at != next.created_at
        || current.ready_at != next.ready_at
        || current.stopped_at != next.stopped_at
        || current.requested_stop_reason != next.requested_stop_reason
        || current.stop_reason != next.stop_reason
        || current.deleted_at != next.deleted_at
        || current.origin != next.origin
}

pub(super) fn conversation_logs(
    mutation: &AgentConversationMutation,
) -> Result<Vec<AgentLogEntry>, SandboxStoreError> {
    let mut seq = mutation.expected_last_seq;
    let mut entries = Vec::with_capacity(mutation.agent_logs.len());
    for kind in &mutation.agent_logs {
        seq = seq
            .checked_add(1)
            .ok_or(SandboxStoreError::SequenceConflict)?;
        entries.push(AgentLogEntry {
            agent_id: mutation.agent.id.clone(),
            seq,
            appended_at: mutation.appended_at.clone(),
            kind: kind.clone(),
        });
    }
    Ok(entries)
}

async fn replace_agent(
    store: &MongoSandboxStore,
    current: &Agent,
    mutation: &AgentConversationMutation,
    integrity: mongodb::bson::Document,
    exact_checkpoint: Option<&layer2_core::sandbox::store::AgentMessageCheckpoint>,
    session: &mut mongodb::ClientSession,
) -> Result<(), ApplyError> {
    let mut fields = agent_replacement_fields(&mutation.agent).map_err(ApplyError::Domain)?;
    if let Some(value) = exact_checkpoint {
        checkpoint::set_field(&mut fields, value).map_err(ApplyError::Domain)?;
    }
    fields.insert("agent_log_integrity_v1", integrity);
    fields.insert(
        "last_seq",
        signed(mutation.agent.conversation.last_seq, "agent log sequence")
            .map_err(SandboxStoreError::Corrupt)
            .map_err(ApplyError::Domain)?,
    );
    let expected_revision = signed(mutation.expected_agent_revision, "agent revision")
        .map_err(|_| ApplyError::Domain(SandboxStoreError::Conflict))?;
    let expected_last_seq = signed(mutation.expected_last_seq, "agent log sequence")
        .map_err(SandboxStoreError::Corrupt)
        .map_err(ApplyError::Domain)?;
    let advanced = store
        .agents
        .update_one(
            doc! {
                "_id": current.id.as_str(),
                "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                "revision": expected_revision,
                "last_seq": expected_last_seq,
                "record.conversation.last_seq": expected_last_seq,
                "channel_history_seal": { "$exists": false },
            },
            doc! { "$set": fields },
        )
        .session(session)
        .await
        .map_err(classify_write)?;
    if advanced.matched_count == 1 {
        Ok(())
    } else {
        Err(ApplyError::Domain(SandboxStoreError::Conflict))
    }
}

async fn insert_logs(
    store: &MongoSandboxStore,
    logs: &[mongodb::bson::RawDocumentBuf],
    session: &mut mongodb::ClientSession,
) -> Result<(), ApplyError> {
    for entry in logs {
        store
            .agent_log
            .clone_with_type::<mongodb::bson::RawDocumentBuf>()
            .insert_one(entry)
            .session(&mut *session)
            .await
            .map_err(|error| {
                if is_duplicate_key(&error) {
                    ApplyError::Domain(SandboxStoreError::SequenceConflict)
                } else {
                    ApplyError::Driver(error)
                }
            })?;
    }
    Ok(())
}

fn classify_write(error: mongodb::error::Error) -> ApplyError {
    if is_duplicate_key(&error) {
        ApplyError::Domain(SandboxStoreError::Duplicate)
    } else {
        ApplyError::Driver(error)
    }
}

#[cfg(test)]
pub(super) async fn reconcile_conversation(
    store: &MongoSandboxStore,
    receipt: &ConversationReceipt,
) -> Result<bool, SandboxStoreError> {
    let mut session = snapshot_session(store, None).await?;
    reconcile_conversation_snapshot(store, receipt, &mut session).await
}

async fn reconcile_conversation_after_commit(
    store: &MongoSandboxStore,
    receipt: &ConversationReceipt,
    snapshot_time: mongodb::bson::Timestamp,
) -> Result<bool, SandboxStoreError> {
    let mut session = snapshot_session(store, Some(snapshot_time)).await?;
    reconcile_conversation_snapshot(store, receipt, &mut session).await
}

pub(super) async fn reconcile_conversation_snapshot(
    store: &MongoSandboxStore,
    receipt: &ConversationReceipt,
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
    if receipt.segmented.is_none() && agent.as_ref() != Some(&receipt.agent) {
        return Ok(false);
    }
    if let Some(dispatch) = &receipt.command_dispatch {
        if !super::agent_commands::binding::matches_snapshot(store, dispatch, session).await? {
            return Ok(false);
        }
    }
    if let Some(recorded) = &receipt.segmented {
        if recorded.next != receipt.agent || recorded.logs != receipt.logs {
            return Ok(false);
        }
        return recorded.matches(store, session).await;
    }
    let Some(first_receipt_seq) = receipt.logs.first().map(|entry| entry.seq) else {
        return Ok(false);
    };
    let mut logs = Vec::with_capacity(receipt.logs.len());
    scan_complete_log_with_snapshot_session(
        ScanCaller::UnknownCommitProof,
        store,
        &receipt.agent.id,
        receipt.agent.conversation.last_seq,
        session,
        |entry| {
            if entry.seq >= first_receipt_seq {
                logs.push(entry);
            }
            Ok(())
        },
    )
    .await
    .map_err(|error| snapshot_error(error.into()))?;
    receipt_logs_match(&logs, receipt)
}

fn receipt_logs_match(
    logs: &[AgentLogEntry],
    receipt: &ConversationReceipt,
) -> Result<bool, SandboxStoreError> {
    if receipt.logs.last().map(|entry| entry.seq) != Some(receipt.agent.conversation.last_seq) {
        return Ok(false);
    }
    if logs.len() != receipt.logs.len() {
        return Ok(false);
    }
    for (actual, expected) in logs.iter().zip(&receipt.logs) {
        if actual.seq != expected.seq {
            return Ok(false);
        }
        if actual != expected {
            return Err(SandboxStoreError::SequenceConflict);
        }
    }
    Ok(true)
}

async fn pending_approvals(
    store: &MongoSandboxStore,
    id: &AgentId,
) -> Result<Vec<AgentLogEntry>, SandboxStoreError> {
    let mut session = snapshot_session(store, None).await?;
    pending_approvals_snapshot(store, id, &mut session).await
}

pub(super) mod reads;
use reads::{pending_approvals_snapshot, snapshot_error, snapshot_session};
