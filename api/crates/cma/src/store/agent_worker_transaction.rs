//! Atomic Mongo publication for one fenced agent work step.

use layer2_core::sandbox::store::{AgentWorkMutation, SandboxStoreError, VersionedWorktree};
use layer2_core::sandbox::{Agent, AgentLogEntry, AgentStatus, SandboxEvent, Worktree};
use mongodb::bson::{doc, Bson};

use super::agent_log::log_document;
use super::agents::{agent_replacement_fields, read_agent};
use super::sandbox_events::{append_in_transaction, TransactionEventError};
use super::sandbox_mongo::{
    commit_transaction, is_duplicate_key, is_transient, record_bson, signed, start_transaction,
    CommitOutcome, MongoSandboxStore, DOCUMENT_SCHEMA_VERSION, TRANSACTION_ATTEMPTS,
};
use super::worktrees::{read_worktree, worktree_replacement_fields};

#[cfg_attr(test, derive(Clone))]
pub(super) struct PublicationReceipt {
    pub(super) agent: Agent,
    pub(super) worktree: Option<Worktree>,
    pub(super) logs: Vec<AgentLogEntry>,
    pub(super) event: Option<SandboxEvent>,
}

enum ApplyError {
    Domain(SandboxStoreError),
    Driver(mongodb::error::Error),
}

impl From<TransactionEventError> for ApplyError {
    fn from(error: TransactionEventError) -> Self {
        match error {
            TransactionEventError::Domain(error) => Self::Domain(error),
            TransactionEventError::Driver(error) => Self::Driver(error),
        }
    }
}

pub(super) async fn commit_agent_work(
    store: &MongoSandboxStore,
    mutation: AgentWorkMutation,
) -> Result<(), SandboxStoreError> {
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
        let receipt = match apply_publication(store, &mutation, &mut session).await {
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
            CommitOutcome::Committed => return Ok(()),
            CommitOutcome::Retry(error) => last_transient = Some(error.to_string()),
            CommitOutcome::Unknown(error) => {
                return if super::agent_worker_reconcile::reconcile_publication(store, &receipt)
                    .await?
                {
                    Ok(())
                } else {
                    Err(SandboxStoreError::Unavailable(format!(
                        "agent work publication commit remained unknown: {error}"
                    )))
                };
            }
            CommitOutcome::Failed(error) => {
                return Err(SandboxStoreError::Unavailable(error.to_string()));
            }
        }
    }
    Err(SandboxStoreError::Unavailable(format!(
        "agent work publication exhausted its retry budget: {}",
        last_transient.unwrap_or_else(|| "no driver result".into())
    )))
}

async fn apply_publication(
    store: &MongoSandboxStore,
    mutation: &AgentWorkMutation,
    session: &mut mongodb::ClientSession,
) -> Result<PublicationReceipt, ApplyError> {
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
    validate_agent_mutation(&current, mutation)?;

    let current_worktree = match mutation.worktree.as_ref() {
        Some(versioned) => {
            let current_worktree = store
                .worktrees
                .find_one(doc! { "_id": versioned.worktree.id.as_str() })
                .session(&mut *session)
                .await
                .map_err(ApplyError::Driver)?
                .as_ref()
                .map(read_worktree)
                .transpose()
                .map_err(ApplyError::Domain)?
                .ok_or(ApplyError::Domain(SandboxStoreError::NotFound))?;
            validate_worktree_mutation(&current, &current_worktree, versioned)?;
            Some(current_worktree)
        }
        None => None,
    };

    let (final_agent, logs) = final_agent_and_logs(&current, mutation)?;
    advance_agent(store, &current, &final_agent, mutation, session).await?;
    if let (Some(current), Some(versioned)) =
        (current_worktree.as_ref(), mutation.worktree.as_ref())
    {
        advance_worktree(store, current, &versioned.worktree, session).await?;
    }
    insert_logs(store, &logs, session).await?;
    let event = match mutation.sandbox_event.as_ref() {
        Some(kind) => Some(
            append_in_transaction(
                store,
                session,
                &final_agent.sandbox_id,
                mutation.appended_at.clone(),
                kind.clone(),
            )
            .await?,
        ),
        None => None,
    };
    Ok(PublicationReceipt {
        agent: final_agent,
        worktree: mutation
            .worktree
            .as_ref()
            .map(|versioned| versioned.worktree.clone()),
        logs,
        event,
    })
}

fn validate_agent_mutation(
    current: &Agent,
    mutation: &AgentWorkMutation,
) -> Result<(), ApplyError> {
    let log_count = u64::try_from(mutation.agent_logs.len())
        .map_err(|_| ApplyError::Domain(SandboxStoreError::SequenceConflict))?;
    let next_last_seq = current
        .conversation
        .last_seq
        .checked_add(log_count)
        .ok_or(ApplyError::Domain(SandboxStoreError::SequenceConflict))?;
    let next_revision = mutation
        .expected_agent_revision
        .checked_add(1)
        .ok_or(ApplyError::Domain(SandboxStoreError::Conflict))?;
    let next = &mutation.agent;
    if current.revision != mutation.expected_agent_revision
        || next.revision != next_revision
        || mutation.holder.trim().is_empty()
        || mutation.fence == 0
        || !current.work.owns(&mutation.holder, mutation.fence)
        || immutable_agent_fields_changed(current, next)
        || next.work.next_fence != current.work.next_fence
        || next
            .work
            .lease
            .as_ref()
            .is_some_and(|lease| current.work.lease.as_ref() != Some(lease))
        || next.conversation.last_seq != next_last_seq
        || next.validate_conversation().is_err()
        || !stop_intent_change_is_valid(current, next)
    {
        return Err(ApplyError::Domain(SandboxStoreError::Conflict));
    }
    Ok(())
}

pub(super) fn stop_intent_change_is_valid(current: &Agent, next: &Agent) -> bool {
    if current.stop_reason.is_some() && next.stop_reason != current.stop_reason {
        return false;
    }
    match current.requested_stop_reason.as_ref() {
        Some(requested) => {
            (current.stop_reason.is_none()
                && next.status == current.status
                && next.requested_stop_reason.as_ref() == Some(requested)
                && next.stop_reason == current.stop_reason)
                || (current.status.is_live()
                    && next.status == AgentStatus::Stopped
                    && next.requested_stop_reason.is_none()
                    && next.stop_reason.as_ref() == Some(requested))
        }
        None => {
            next.requested_stop_reason.is_none()
                && (next.stop_reason == current.stop_reason
                    || (current.stop_reason.is_none()
                        && current.status.is_live()
                        && next.status == AgentStatus::Stopped
                        && next.stop_reason.is_some()))
        }
    }
}

fn immutable_agent_fields_changed(current: &Agent, next: &Agent) -> bool {
    current.id != next.id
        || current.worktree_id != next.worktree_id
        || current.workspace_id != next.workspace_id
        || current.sandbox_id != next.sandbox_id
        || current.owner != next.owner
        || current.creation_key != next.creation_key
        || current.creation_fingerprint != next.creation_fingerprint
        || current.profile != next.profile
        || current.created_at != next.created_at
        || current.origin != next.origin
}

fn validate_worktree_mutation(
    agent: &Agent,
    current: &Worktree,
    versioned: &VersionedWorktree,
) -> Result<(), ApplyError> {
    let next = &versioned.worktree;
    if current.id != agent.worktree_id
        || current.revision != versioned.expected_revision
        || next.revision != versioned.expected_revision.checked_add(1).unwrap_or(0)
        || current.id != next.id
        || current.workspace_id != next.workspace_id
        || current.sandbox_id != next.sandbox_id
        || current.owner != next.owner
        || current.name != next.name
        || current.path != next.path
        || current.branch != next.branch
        || current.base_commit != next.base_commit
        || current.agent_id != next.agent_id
        || current.status == next.status
        || current.status.transition(next.status).is_err()
    {
        return Err(ApplyError::Domain(SandboxStoreError::Conflict));
    }
    Ok(())
}

fn final_agent_and_logs(
    current: &Agent,
    mutation: &AgentWorkMutation,
) -> Result<(Agent, Vec<AgentLogEntry>), ApplyError> {
    let final_agent = mutation.agent.clone();
    final_agent
        .validate_conversation()
        .map_err(|_| ApplyError::Domain(SandboxStoreError::Conflict))?;
    let logs = mutation
        .agent_logs
        .iter()
        .enumerate()
        .map(|(offset, kind)| AgentLogEntry {
            agent_id: final_agent.id.clone(),
            seq: current.conversation.last_seq + offset as u64 + 1,
            appended_at: mutation.appended_at.clone(),
            kind: kind.clone(),
        })
        .collect();
    Ok((final_agent, logs))
}

async fn advance_agent(
    store: &MongoSandboxStore,
    current: &Agent,
    next: &Agent,
    mutation: &AgentWorkMutation,
    session: &mut mongodb::ClientSession,
) -> Result<(), ApplyError> {
    let lease = current
        .work
        .lease
        .as_ref()
        .ok_or(ApplyError::Domain(SandboxStoreError::Conflict))?;
    let mut fields = agent_replacement_fields(next).map_err(ApplyError::Domain)?;
    fields.insert(
        "last_seq",
        signed(next.conversation.last_seq, "agent log sequence")
            .map_err(SandboxStoreError::Corrupt)
            .map_err(ApplyError::Domain)?,
    );
    let advanced = store
        .agents
        .update_one(
            doc! {
                "_id": current.id.as_str(),
                "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                "revision": signed(mutation.expected_agent_revision, "agent revision")
                    .map_err(|_| ApplyError::Domain(SandboxStoreError::Conflict))?,
                "last_seq": signed(current.conversation.last_seq, "agent log sequence")
                    .map_err(SandboxStoreError::Corrupt)
                    .map_err(ApplyError::Domain)?,
                "work_lease_holder": &mutation.holder,
                "work_lease_fence": signed(mutation.fence, "agent work lease fence")
                    .map_err(|_| ApplyError::Domain(SandboxStoreError::Conflict))?,
                "work_lease_expires_at": &lease.expires_at,
                "work_next_fence": signed(current.work.next_fence, "agent work next fence")
                    .map_err(|_| ApplyError::Domain(SandboxStoreError::Conflict))?,
            },
            doc! { "$set": fields },
        )
        .session(session)
        .await
        .map_err(classify_aggregate_write)?;
    if advanced.matched_count == 1 {
        Ok(())
    } else {
        Err(ApplyError::Domain(SandboxStoreError::Conflict))
    }
}

async fn advance_worktree(
    store: &MongoSandboxStore,
    current: &Worktree,
    next: &Worktree,
    session: &mut mongodb::ClientSession,
) -> Result<(), ApplyError> {
    let agent_id = current
        .agent_id
        .as_ref()
        .map(|id| Bson::String(id.as_str().to_owned()))
        .unwrap_or(Bson::Null);
    let advanced = store
        .worktrees
        .update_one(
            doc! {
                "_id": current.id.as_str(),
                "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                "workspace_id": current.workspace_id.as_str(),
                "sandbox_id": current.sandbox_id.as_str(),
                "owner": current.owner.as_str(),
                "name": &current.name,
                "status": record_bson(&current.status)
                    .map_err(SandboxStoreError::Corrupt)
                    .map_err(ApplyError::Domain)?,
                "revision": signed(current.revision, "worktree revision")
                    .map_err(|_| ApplyError::Domain(SandboxStoreError::Conflict))?,
                "record.path": &current.path,
                "record.branch": &current.branch,
                "record.base_commit": &current.base_commit,
                "record.agent_id": agent_id,
            },
            doc! { "$set": worktree_replacement_fields(next).map_err(ApplyError::Domain)? },
        )
        .session(session)
        .await
        .map_err(classify_aggregate_write)?;
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

fn classify_aggregate_write(error: mongodb::error::Error) -> ApplyError {
    if is_duplicate_key(&error) {
        ApplyError::Domain(SandboxStoreError::Duplicate)
    } else {
        ApplyError::Driver(error)
    }
}
