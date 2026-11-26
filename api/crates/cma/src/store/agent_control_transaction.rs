//! Majority-durable Mongo execution for agent control plans.

use crate::store::transaction_terminal::GuardedTerminal as _;
use layer2_core::sandbox::store::{
    AgentControlResult, AgentControlStoreError, AgentDeleteRequest, AgentStopRequest,
    SandboxStoreError,
};
use layer2_core::sandbox::Agent;
use mongodb::bson::doc;

use super::agent_control_lineage::{
    fence_control_lineage, load_control_snapshot, ControlSnapshot, LineageError,
};
use super::agent_control_receipt::{ControlOperation, ControlRequest};
use super::agent_log_integrity::carry_integrity_for_revision;
use super::agents::agent_replacement_fields;
use super::sandbox_mongo::{
    commit_transaction, is_transient, signed, start_transaction, wait_for_reconciliation,
    CommitOutcome, MongoSandboxStore, DOCUMENT_SCHEMA_VERSION,
};

const CONTROL_ATTEMPTS: usize = 8;
pub(super) mod restart;
pub(super) mod restart_cancel;
pub(super) mod restart_dispatch;
mod restart_publication;
mod restart_receipt;
pub(super) mod restart_teardown;
mod segmented;
pub(super) use restart::schedule_restart;
pub(super) use restart_dispatch::{
    live_lease_expression as restart_live_lease_expression, load_restart_connection,
    load_restart_work, prepare_restart_dispatch,
};

enum ApplyError {
    Domain(AgentControlStoreError),
    Driver(mongodb::error::Error),
}

enum ControlAttempt {
    ReadOnly(Box<Result<AgentControlResult, AgentControlStoreError>>),
    Write {
        current: Box<ControlSnapshot>,
        next: Box<Agent>,
    },
}

impl From<LineageError> for ApplyError {
    fn from(error: LineageError) -> Self {
        match error {
            LineageError::Domain(error) => Self::Domain(error),
            LineageError::ReceiptCorrupt(detail) => {
                Self::Domain(AgentControlStoreError::Corrupt(detail))
            }
            LineageError::Driver(error) => Self::Driver(error),
        }
    }
}

pub(super) async fn schedule_stop(
    store: &MongoSandboxStore,
    request: AgentStopRequest,
) -> Result<AgentControlResult, AgentControlStoreError> {
    schedule_control(store, ControlRequest::Stop(request)).await
}

pub(super) async fn schedule_delete(
    store: &MongoSandboxStore,
    request: AgentDeleteRequest,
) -> Result<AgentControlResult, AgentControlStoreError> {
    schedule_control(store, ControlRequest::Delete(request)).await
}

async fn schedule_control(
    store: &MongoSandboxStore,
    request: ControlRequest,
) -> Result<AgentControlResult, AgentControlStoreError> {
    let operation = ControlOperation::new(request);
    let row = store
        .agents
        .find_one(doc! {"_id":operation.request.target().agent_id.as_str()})
        .await
        .map_err(unavailable)?
        .ok_or(AgentControlStoreError::NotFound)?;
    if operation.request.target().expected_status == layer2_core::sandbox::AgentStatus::Starting
        && row.contains_key(restart::ACTIVE_RESTART_FIELD)
    {
        return Box::pin(restart_cancel::schedule(store, &operation)).await;
    }
    if super::agent_log_segments::read_format(&row)
        .map_err(store_error)?
        .is_some()
    {
        return segmented::schedule(store, &operation).await;
    }
    let mut last_transient = None;
    let mut last_was_conflict = false;
    for attempt in 0..CONTROL_ATTEMPTS {
        if attempt > 0 {
            tokio::task::yield_now().await;
        }
        let mut session = match store.client.start_session().await {
            Ok(session) => session,
            Err(error) if is_transient(&error) => {
                last_was_conflict = false;
                last_transient = Some(error.to_string());
                continue;
            }
            Err(error) => return Err(unavailable(error)),
        };
        if let Err(error) = start_transaction(&mut session).await {
            if is_transient(&error) {
                last_was_conflict = false;
                last_transient = Some(error.to_string());
                continue;
            }
            return Err(unavailable(error));
        }
        let attempt =
            match load_control_snapshot(store, operation.request.target(), &mut session, false)
                .await
                .map_err(ApplyError::from)
            {
                Ok(current) => match operation.request.plan(current.agent.clone()) {
                    Ok(AgentControlResult::Scheduled(_))
                        if current.sandbox.status
                            == layer2_core::sandbox::SandboxStatus::Rotating =>
                    {
                        ControlAttempt::ReadOnly(Box::new(Err(AgentControlStoreError::Conflict)))
                    }
                    Ok(AgentControlResult::Scheduled(next)) => ControlAttempt::Write {
                        current: Box::new(current),
                        next: Box::new(next),
                    },
                    read => ControlAttempt::ReadOnly(Box::new(read)),
                },
                Err(ApplyError::Domain(error)) => ControlAttempt::ReadOnly(Box::new(Err(error))),
                Err(ApplyError::Driver(error)) if is_transient(&error) => {
                    let _ = session.abort_guarded().await;
                    last_was_conflict = false;
                    last_transient = Some(error.to_string());
                    continue;
                }
                Err(ApplyError::Driver(error)) => {
                    let _ = session.abort_guarded().await;
                    return Err(unavailable(error));
                }
            };
        match attempt {
            ControlAttempt::ReadOnly(result) => match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return *result,
                CommitOutcome::Retry(error) | CommitOutcome::Unknown(error) => {
                    last_was_conflict = false;
                    last_transient = Some(error.to_string());
                }
                CommitOutcome::Failed(error) => return Err(unavailable(error)),
            },
            ControlAttempt::Write { current, next } => {
                match replace_agent(store, &current, &next, &operation, &mut session).await {
                    Ok(true) => {}
                    Ok(false) => {
                        let _ = session.abort_guarded().await;
                        last_was_conflict = true;
                        continue;
                    }
                    Err(ApplyError::Domain(error)) => {
                        let _ = session.abort_guarded().await;
                        return Err(error);
                    }
                    Err(ApplyError::Driver(error)) if is_transient(&error) => {
                        let _ = session.abort_guarded().await;
                        last_was_conflict = false;
                        last_transient = Some(error.to_string());
                        continue;
                    }
                    Err(ApplyError::Driver(error)) => {
                        let _ = session.abort_guarded().await;
                        return Err(unavailable(error));
                    }
                }
                match commit_transaction(&mut session).await {
                    CommitOutcome::Committed => {
                        return Ok(AgentControlResult::Scheduled(*next));
                    }
                    CommitOutcome::Retry(error) => {
                        last_was_conflict = false;
                        last_transient = Some(error.to_string());
                    }
                    CommitOutcome::Unknown(error) => {
                        let snapshot_time = session.operation_time().ok_or_else(|| {
                            AgentControlStoreError::Unavailable(format!(
                                "agent control commit remained unknown without an operation time: {error}"
                            ))
                        })?;
                        let commit_error = error.to_string();
                        let store = store.clone();
                        return wait_for_reconciliation("agent control commit", async move {
                            reconcile_control(&store, &operation, &next, None, snapshot_time, error)
                                .await
                        })
                        .await
                        .map_err(|deadline| {
                            AgentControlStoreError::Unavailable(format!(
                                "{deadline}; ambiguous commit: {commit_error}"
                            ))
                        })?;
                    }
                    CommitOutcome::Failed(error) => return Err(unavailable(error)),
                }
            }
        }
    }
    if last_was_conflict {
        return Err(AgentControlStoreError::Conflict);
    }
    Err(AgentControlStoreError::Unavailable(format!(
        "agent control transaction exhausted its retry budget: {}",
        last_transient.unwrap_or_else(|| "no driver result".into())
    )))
}

async fn reconcile_control(
    store: &MongoSandboxStore,
    operation: &ControlOperation,
    expected: &Agent,
    recorded: Option<&super::agent_log_segments::writer_commit::Recorded>,
    snapshot_time: mongodb::bson::Timestamp,
    commit_error: mongodb::error::Error,
) -> Result<AgentControlResult, AgentControlStoreError> {
    let mut session = store
        .client
        .start_session()
        .snapshot(true)
        .snapshot_time(snapshot_time)
        .await
        .map_err(unavailable)?;
    let snapshot = load_control_snapshot(store, operation.request.target(), &mut session, true)
        .await
        .map_err(|error| reconciliation_error(error, &commit_error))?;
    if !control_snapshot_matches(operation, expected, &snapshot) {
        return Err(unknown_control(commit_error));
    }
    if let Some(recorded) = recorded {
        if recorded.next != *expected
            || !recorded
                .matches(store, &mut session)
                .await
                .map_err(store_error)?
        {
            return Err(unknown_control(commit_error));
        }
    }
    Ok(AgentControlResult::Scheduled(snapshot.agent))
}

pub(super) fn control_snapshot_matches(
    operation: &ControlOperation,
    expected: &Agent,
    snapshot: &ControlSnapshot,
) -> bool {
    let current = &snapshot.agent;
    operation.was_recorded(&snapshot.receipts, expected)
        && same_agent_incarnation(current, expected)
        && current.revision >= expected.revision
        && (current == expected || operation.request.durable_replay(current, expected))
}

pub(super) fn same_agent_incarnation(current: &Agent, expected: &Agent) -> bool {
    current.id == expected.id
        && current.worktree_id == expected.worktree_id
        && current.workspace_id == expected.workspace_id
        && current.sandbox_id == expected.sandbox_id
        && current.owner == expected.owner
        && current.creation_key == expected.creation_key
        && current.creation_fingerprint == expected.creation_fingerprint
        && current.profile == expected.profile
        && current.created_at == expected.created_at
        && current.origin == expected.origin
}

async fn replace_agent(
    store: &MongoSandboxStore,
    current: &ControlSnapshot,
    next: &Agent,
    operation: &ControlOperation,
    session: &mut mongodb::ClientSession,
) -> Result<bool, ApplyError> {
    let agent = &current.agent;
    let revision = signed(agent.revision, "agent revision")
        .map_err(|_| ApplyError::Domain(AgentControlStoreError::Conflict))?;
    let last_seq = signed(agent.conversation.last_seq, "agent log sequence")
        .map_err(AgentControlStoreError::Corrupt)
        .map_err(ApplyError::Domain)?;
    let mut filter = doc! {
        "_id": agent.id.as_str(),
        "document_schema_version": DOCUMENT_SCHEMA_VERSION,
        "revision": revision,
        "last_seq": last_seq,
        "record.conversation.last_seq": last_seq,
        "channel_reuse_cleanup_execution": { "$exists": false },
                "channel_history_seal": { "$exists": false },
    };
    filter.insert(operation.receipt_field(), doc! { "$exists": false });
    let mut fields = agent_replacement_fields(next)
        .map_err(store_error)
        .map_err(ApplyError::Domain)?;
    let current_row = store
        .agents
        .find_one(doc! { "_id": agent.id.as_str() })
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?
        .ok_or(ApplyError::Domain(AgentControlStoreError::NotFound))?;
    let current_agent = super::agents::read_agent(&current_row)
        .map_err(store_error)
        .map_err(ApplyError::Domain)?;
    carry_integrity_for_revision(&current_row, &current_agent, next, &mut fields)
        .map_err(store_error)
        .map_err(ApplyError::Domain)?;
    fields.extend(operation.receipt_update(next).map_err(ApplyError::Domain)?);
    let result = store
        .agents
        .update_one(filter, doc! { "$set": fields })
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?;
    if result.matched_count != 1 {
        return Ok(false);
    }
    match fence_control_lineage(store, current, operation.id(), session).await {
        Ok(()) => Ok(true),
        Err(LineageError::Domain(AgentControlStoreError::Conflict)) => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn store_error(error: SandboxStoreError) -> AgentControlStoreError {
    match error {
        SandboxStoreError::NotFound => AgentControlStoreError::NotFound,
        SandboxStoreError::Corrupt(detail) => AgentControlStoreError::Corrupt(detail),
        other => AgentControlStoreError::Unavailable(other.to_string()),
    }
}

fn unavailable(error: mongodb::error::Error) -> AgentControlStoreError {
    AgentControlStoreError::Unavailable(error.to_string())
}

pub(super) fn reconciliation_error(
    error: LineageError,
    commit_error: &mongodb::error::Error,
) -> AgentControlStoreError {
    match error {
        LineageError::ReceiptCorrupt(detail) => AgentControlStoreError::Corrupt(detail),
        LineageError::Domain(_) => unknown_control(commit_error),
        LineageError::Driver(error) => AgentControlStoreError::Unavailable(format!(
            "agent control commit remained unknown: {commit_error}; reconciliation failed: {error}"
        )),
    }
}

fn unknown_control(error: impl std::fmt::Display) -> AgentControlStoreError {
    AgentControlStoreError::Unavailable(format!("agent control commit remained unknown: {error}"))
}
