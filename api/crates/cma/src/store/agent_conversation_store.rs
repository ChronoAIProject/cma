//! Atomic Mongo persistence for runtime conversation updates.

use std::collections::BTreeMap;

use futures_util::TryStreamExt;
use layer2_core::ids::AgentId;
use layer2_core::sandbox::store::{
    AgentConversationMutation, AgentConversationStore, SandboxStoreError,
};
use layer2_core::sandbox::{Agent, AgentLogEntry, AgentLogKind};
use layer2_core::seconds_from_rfc3339;
use mongodb::bson::doc;

use super::agent_log::{log_document, read_entry, reconcile_entry};
use super::agents::{agent_replacement_fields, read_agent};
use super::sandbox_mongo::{
    commit_transaction, is_duplicate_key, is_transient, signed, start_transaction, CommitOutcome,
    MongoSandboxStore, DOCUMENT_SCHEMA_VERSION, TRANSACTION_ATTEMPTS,
};

#[cfg_attr(test, derive(Clone))]
pub(super) struct ConversationReceipt {
    pub(super) agent: Agent,
    pub(super) logs: Vec<AgentLogEntry>,
}

enum ApplyError {
    Domain(SandboxStoreError),
    Driver(mongodb::error::Error),
}

#[async_trait::async_trait]
impl AgentConversationStore for MongoSandboxStore {
    async fn commit_agent_conversation(
        &self,
        mutation: AgentConversationMutation,
    ) -> Result<Vec<AgentLogEntry>, SandboxStoreError> {
        commit_conversation(self, mutation).await
    }

    async fn pending_agent_approvals(
        &self,
        id: &AgentId,
    ) -> Result<Vec<AgentLogEntry>, SandboxStoreError> {
        pending_approvals(self, id).await
    }
}

async fn commit_conversation(
    store: &MongoSandboxStore,
    mutation: AgentConversationMutation,
) -> Result<Vec<AgentLogEntry>, SandboxStoreError> {
    let mut last_transient = None;
    for attempt in 0..TRANSACTION_ATTEMPTS {
        if attempt > 0 {
            tokio::task::yield_now().await;
        }
        let mut session = match store.client.start_session().await {
            Ok(session) => session,
            Err(error) if is_transient(&error) => {
                last_transient = Some(error.to_string());
                continue;
            }
            Err(error) => return Err(SandboxStoreError::Unavailable(error.to_string())),
        };
        if let Err(error) = start_transaction(&mut session).await {
            if is_transient(&error) {
                last_transient = Some(error.to_string());
                continue;
            }
            return Err(SandboxStoreError::Unavailable(error.to_string()));
        }
        let receipt = match apply_mutation(store, &mutation, &mut session).await {
            Ok(receipt) => receipt,
            Err(ApplyError::Domain(error)) => {
                let _ = session.abort_transaction().await;
                return Err(error);
            }
            Err(ApplyError::Driver(error)) if is_transient(&error) => {
                let _ = session.abort_transaction().await;
                last_transient = Some(error.to_string());
                continue;
            }
            Err(ApplyError::Driver(error)) => {
                let _ = session.abort_transaction().await;
                return Err(SandboxStoreError::Unavailable(error.to_string()));
            }
        };
        match commit_transaction(&mut session).await {
            CommitOutcome::Committed => return Ok(receipt.logs),
            CommitOutcome::Retry(error) => last_transient = Some(error.to_string()),
            CommitOutcome::Unknown(error) => {
                return if reconcile_conversation(store, &receipt).await? {
                    Ok(receipt.logs)
                } else {
                    Err(SandboxStoreError::Unavailable(format!(
                        "agent conversation commit remained unknown: {error}"
                    )))
                };
            }
            CommitOutcome::Failed(error) => {
                return Err(SandboxStoreError::Unavailable(error.to_string()));
            }
        }
    }
    Err(SandboxStoreError::Unavailable(format!(
        "agent conversation transaction exhausted its retry budget: {}",
        last_transient.unwrap_or_else(|| "no driver result".into())
    )))
}

async fn apply_mutation(
    store: &MongoSandboxStore,
    mutation: &AgentConversationMutation,
    session: &mut mongodb::ClientSession,
) -> Result<ConversationReceipt, ApplyError> {
    let current = store
        .agents
        .find_one(doc! { "_id": mutation.agent.id.as_str() })
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?
        .as_ref()
        .map(read_agent)
        .transpose()
        .map_err(ApplyError::Domain)?
        .ok_or(ApplyError::Domain(SandboxStoreError::NotFound))?;
    let actual_last_seq = transaction_log_tail(store, &mutation.agent.id, session).await?;
    validate_mutation(&current, actual_last_seq, mutation).map_err(ApplyError::Domain)?;
    let logs = conversation_logs(mutation).map_err(ApplyError::Domain)?;

    replace_agent(store, &current, mutation, session).await?;
    insert_logs(store, &logs, session).await?;
    Ok(ConversationReceipt {
        agent: mutation.agent.clone(),
        logs,
    })
}

async fn transaction_log_tail(
    store: &MongoSandboxStore,
    id: &AgentId,
    session: &mut mongodb::ClientSession,
) -> Result<u64, ApplyError> {
    store
        .agent_log
        .find_one(doc! { "agent_id": id.as_str() })
        .sort(doc! { "seq": -1 })
        .session(session)
        .await
        .map_err(ApplyError::Driver)?
        .as_ref()
        .map(read_entry)
        .transpose()
        .map_err(ApplyError::Domain)
        .map(|entry| entry.map_or(0, |entry| entry.seq))
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
        || current.work != next.work
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

fn conversation_logs(
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
    session: &mut mongodb::ClientSession,
) -> Result<(), ApplyError> {
    let mut fields = agent_replacement_fields(&mutation.agent).map_err(ApplyError::Domain)?;
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
    logs: &[AgentLogEntry],
    session: &mut mongodb::ClientSession,
) -> Result<(), ApplyError> {
    for entry in logs {
        store
            .agent_log
            .insert_one(log_document(entry).map_err(ApplyError::Domain)?)
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

pub(super) async fn reconcile_conversation(
    store: &MongoSandboxStore,
    receipt: &ConversationReceipt,
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
    for entry in &receipt.logs {
        if reconcile_entry(store, entry).await?.is_none() {
            return Ok(false);
        }
    }
    Ok(true)
}

async fn pending_approvals(
    store: &MongoSandboxStore,
    id: &AgentId,
) -> Result<Vec<AgentLogEntry>, SandboxStoreError> {
    let agent = store
        .agents
        .find_one(doc! { "_id": id.as_str() })
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
        .as_ref()
        .map(read_agent)
        .transpose()?
        .ok_or(SandboxStoreError::NotFound)?;
    let high_water = signed(agent.conversation.last_seq, "agent log high water")
        .map_err(SandboxStoreError::Corrupt)?;
    let mut cursor = store
        .agent_log
        .find(doc! { "agent_id": id.as_str(), "seq": { "$lte": high_water } })
        .sort(doc! { "seq": 1 })
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
    let mut expected_seq = 0_u64;
    let mut pending = BTreeMap::new();
    while let Some(document) = cursor
        .try_next()
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
    {
        let entry = read_entry(&document)?;
        expected_seq = expected_seq
            .checked_add(1)
            .ok_or(SandboxStoreError::SequenceConflict)?;
        if entry.seq != expected_seq {
            return Err(SandboxStoreError::SequenceConflict);
        }
        fold_approval(&mut pending, entry);
    }
    if expected_seq != agent.conversation.last_seq {
        return Err(SandboxStoreError::SequenceConflict);
    }
    Ok(pending.into_values().collect())
}

fn fold_approval(pending: &mut BTreeMap<String, AgentLogEntry>, entry: AgentLogEntry) {
    match &entry.kind {
        AgentLogKind::TurnStarted { .. } | AgentLogKind::TurnCompleted { .. } => pending.clear(),
        AgentLogKind::ApprovalRequested { approval_id, .. } => {
            pending.insert(approval_id.clone(), entry);
        }
        AgentLogKind::ApprovalResolved { approval_id, .. } => {
            pending.remove(approval_id);
        }
        AgentLogKind::TurnFailed { .. }
        | AgentLogKind::TurnInterrupted { .. }
        | AgentLogKind::Stopped { .. }
        | AgentLogKind::Deleted => pending.clear(),
        _ => {}
    }
}
