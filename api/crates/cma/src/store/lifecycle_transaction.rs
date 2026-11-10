//! Atomic Mongo publication for one sandbox lifecycle phase.

use crate::store::agent_log_telemetry::ScanCaller;
use std::collections::BTreeMap;

use layer2_core::sandbox::store::{
    validate_lifecycle_mutation, SandboxLifecycleAgentMutation, SandboxLifecycleMutation,
    SandboxStoreError,
};
use layer2_core::sandbox::{Agent, AgentLogEntry, Sandbox, SandboxEvent};
use mongodb::bson::{doc, Bson, Timestamp};
use mongodb::options::ReadConcern;

use super::agent_log::{log_document, read_entry, scan_complete_log_with_session};
use super::agent_log_integrity::{ensure_append_capacity, AppendCapacity, TransactionLogError};
use super::agents::{agent_replacement_fields, read_agent};
use super::lifecycle_execution::{load_sandbox_agents, LifecycleLoadError};
use super::sandbox_events::{
    append_in_transaction, reconcile_event_with_snapshot_session, TransactionEventError,
};
use super::sandbox_mongo::{
    commit_transaction, is_duplicate_key, is_transient, signed, start_transaction,
    wait_for_reconciliation, CommitOutcome, MongoSandboxStore, DOCUMENT_SCHEMA_VERSION,
    TRANSACTION_ATTEMPTS,
};
use super::sandboxes::{read_sandbox, sandbox_replacement_fields};

#[derive(Clone)]
struct LifecycleReceipt {
    sandbox: Sandbox,
    agents: Vec<Agent>,
    logs: Vec<AgentLogEntry>,
    events: Vec<SandboxEvent>,
}

enum ApplyError {
    Domain(SandboxStoreError),
    Driver(mongodb::error::Error),
}

impl From<LifecycleLoadError> for ApplyError {
    fn from(error: LifecycleLoadError) -> Self {
        match error {
            LifecycleLoadError::Domain(error) => Self::Domain(error),
            LifecycleLoadError::Driver(error) => Self::Driver(error),
        }
    }
}

impl From<TransactionEventError> for ApplyError {
    fn from(error: TransactionEventError) -> Self {
        match error {
            TransactionEventError::Domain(error) => Self::Domain(error),
            TransactionEventError::Driver(error) => Self::Driver(error),
        }
    }
}

impl From<TransactionLogError> for ApplyError {
    fn from(error: TransactionLogError) -> Self {
        match error {
            TransactionLogError::Domain(error) => Self::Domain(error),
            TransactionLogError::Driver(error) => Self::Driver(error),
        }
    }
}

pub(super) async fn commit_lifecycle(
    store: &MongoSandboxStore,
    mutation: SandboxLifecycleMutation,
) -> Result<(), SandboxStoreError> {
    let mut last_transient = None;
    for attempt in 0..TRANSACTION_ATTEMPTS {
        if attempt > 0 {
            // A competing writer needs time to settle. A yield alone can burn
            // every attempt while that same transaction still owns the row.
            tokio::time::sleep(std::time::Duration::from_millis(10 << (attempt - 1).min(3))).await;
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
        let applied = apply_lifecycle(store, &mutation, &mut session).await;
        let receipt = match applied {
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
                let snapshot_time = session.operation_time().ok_or_else(|| {
                    SandboxStoreError::Unavailable(format!(
                        "sandbox lifecycle commit remained unknown without an operation time: {error}"
                    ))
                })?;
                let commit_error = error.to_string();
                let unresolved = commit_error.clone();
                let store = store.clone();
                return wait_for_reconciliation("sandbox lifecycle commit", async move {
                    if reconcile_lifecycle(&store, &receipt, snapshot_time).await? {
                        Ok(())
                    } else {
                        Err(SandboxStoreError::Unavailable(format!(
                            "sandbox lifecycle commit remained unknown: {unresolved}"
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
    // Every path reaching here either failed before commit or was explicitly
    // aborted by Mongo. Unknown commits return through reconciliation above.
    // Expose definite contention so the worker can release only its unchanged
    // lifecycle lease and re-observe; do not strand the long capture lease.
    tracing::warn!(
        reason = last_transient.as_deref().unwrap_or("no driver result"),
        "Sandbox lifecycle contention exhausted the bounded retry budget"
    );
    Err(SandboxStoreError::Conflict)
}

async fn apply_lifecycle(
    store: &MongoSandboxStore,
    mutation: &SandboxLifecycleMutation,
    session: &mut mongodb::ClientSession,
) -> Result<LifecycleReceipt, ApplyError> {
    let current = load_sandbox(store, &mutation.guard.sandbox_id, session).await?;
    let agents = load_sandbox_agents(store, &current.id, session).await?;
    super::checkpoint_capture::validate_and_fence_begin(
        store,
        &current,
        &mutation.sandbox,
        &agents,
        session,
    )
    .await?;
    let by_id = agents
        .iter()
        .map(|agent| (agent.id.clone(), agent))
        .collect::<BTreeMap<_, _>>();
    let mut rows = agents
        .iter()
        .cloned()
        .map(|agent| {
            let last_seq = agent.conversation.last_seq;
            (agent, last_seq)
        })
        .collect::<Vec<_>>();
    let mut logs = Vec::new();
    for change in &mutation.agents {
        let stored = by_id
            .get(&change.agent.id)
            .copied()
            .ok_or(ApplyError::Domain(SandboxStoreError::NotFound))?;
        let complete = scan_complete_log_with_session(
            ScanCaller::CommandAdmission,
            store,
            &stored.id,
            stored.conversation.last_seq,
            session,
            |_| Ok(()),
        )
        .await?;
        let batch = materialize_logs(stored, change, &mutation.appended_at)?;
        ensure_append_capacity(&complete, &batch, AppendCapacity::StopOrFailure)
            .map_err(ApplyError::Domain)?;
        let row = rows
            .iter_mut()
            .find(|(agent, _)| agent.id == stored.id)
            .expect("loaded lifecycle agent has a validation row");
        row.1 = complete.entry_count();
        logs.extend(batch);
    }
    validate_lifecycle_mutation(&current, &rows, mutation).map_err(ApplyError::Domain)?;

    advance_sandbox(store, &current, &mutation.sandbox, session).await?;
    for change in &mutation.agents {
        let stored = by_id
            .get(&change.agent.id)
            .copied()
            .expect("validated lifecycle agent exists");
        advance_agent(store, stored, change, session).await?;
    }
    for entry in &logs {
        store
            .agent_log
            .insert_one(log_document(entry).map_err(ApplyError::Domain)?)
            .session(&mut *session)
            .await
            .map_err(classify_log_write)?;
    }
    let mut events = Vec::with_capacity(mutation.events.len());
    for kind in &mutation.events {
        events.push(
            append_in_transaction(
                store,
                session,
                &current.id,
                mutation.appended_at.clone(),
                kind.clone(),
            )
            .await?,
        );
    }
    Ok(LifecycleReceipt {
        sandbox: mutation.sandbox.clone(),
        agents: mutation
            .agents
            .iter()
            .map(|change| change.agent.clone())
            .collect(),
        logs,
        events,
    })
}

async fn load_sandbox(
    store: &MongoSandboxStore,
    id: &layer2_core::SandboxId,
    session: &mut mongodb::ClientSession,
) -> Result<Sandbox, ApplyError> {
    store
        .sandboxes
        .find_one(doc! { "_id": id.as_str() })
        .session(session)
        .await
        .map_err(ApplyError::Driver)?
        .as_ref()
        .map(read_sandbox)
        .transpose()
        .map_err(ApplyError::Domain)?
        .ok_or(ApplyError::Domain(SandboxStoreError::NotFound))
}

fn materialize_logs(
    current: &Agent,
    change: &SandboxLifecycleAgentMutation,
    appended_at: &str,
) -> Result<Vec<AgentLogEntry>, ApplyError> {
    change
        .logs
        .iter()
        .enumerate()
        .map(|(offset, kind)| {
            let offset = u64::try_from(offset)
                .map_err(|_| ApplyError::Domain(SandboxStoreError::SequenceConflict))?;
            let seq = current
                .conversation
                .last_seq
                .checked_add(offset)
                .and_then(|seq| seq.checked_add(1))
                .ok_or(ApplyError::Domain(SandboxStoreError::SequenceConflict))?;
            Ok(AgentLogEntry {
                agent_id: current.id.clone(),
                seq,
                appended_at: appended_at.to_owned(),
                kind: kind.clone(),
            })
        })
        .collect()
}

async fn advance_sandbox(
    store: &MongoSandboxStore,
    current: &Sandbox,
    next: &Sandbox,
    session: &mut mongodb::ClientSession,
) -> Result<(), ApplyError> {
    let advanced = store
        .sandboxes
        .update_one(
            doc! {
                "_id": current.id.as_str(),
                "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                "channel_reuse_cleanup_execution": { "$exists": false },
                "revision": signed(current.revision, "sandbox lifecycle revision")
                    .map_err(|_| ApplyError::Domain(SandboxStoreError::Conflict))?,
            },
            doc! { "$set": sandbox_replacement_fields(next).map_err(ApplyError::Domain)? },
        )
        .session(&mut *session)
        .await
        .map_err(classify_aggregate_write)?;
    if advanced.matched_count == 1 {
        crate::store::sandbox_usage::save::record_saved(store, session, next, current.revision)
            .await?;
        Ok(())
    } else {
        Err(ApplyError::Domain(SandboxStoreError::Conflict))
    }
}

async fn advance_agent(
    store: &MongoSandboxStore,
    current: &Agent,
    change: &SandboxLifecycleAgentMutation,
    session: &mut mongodb::ClientSession,
) -> Result<(), ApplyError> {
    let mut fields = agent_replacement_fields(&change.agent).map_err(ApplyError::Domain)?;
    let row = store
        .agents
        .find_one(doc! { "_id": current.id.as_str() })
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?
        .ok_or(ApplyError::Domain(SandboxStoreError::NotFound))?;
    super::agent_conversation_store::carry_checkpoint_fields(
        &row,
        current,
        &change.agent,
        &mut fields,
    )
    .map_err(ApplyError::Domain)?;
    fields.insert(
        "last_seq",
        Bson::Int64(
            signed(
                change.agent.conversation.last_seq,
                "lifecycle agent log sequence",
            )
            .map_err(SandboxStoreError::Corrupt)
            .map_err(ApplyError::Domain)?,
        ),
    );
    let advanced = store
        .agents
        .update_one(
            doc! {
                "_id": current.id.as_str(),
                "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                "sandbox_id": current.sandbox_id.as_str(),
                "owner": current.owner.as_str(),
                "revision": signed(current.revision, "lifecycle agent revision")
                    .map_err(|_| ApplyError::Domain(SandboxStoreError::Conflict))?,
                "last_seq": signed(current.conversation.last_seq, "lifecycle agent log sequence")
                    .map_err(SandboxStoreError::Corrupt)
                    .map_err(ApplyError::Domain)?,
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

fn classify_aggregate_write(error: mongodb::error::Error) -> ApplyError {
    if is_duplicate_key(&error) {
        ApplyError::Domain(SandboxStoreError::Duplicate)
    } else {
        ApplyError::Driver(error)
    }
}

fn classify_log_write(error: mongodb::error::Error) -> ApplyError {
    if is_duplicate_key(&error) {
        ApplyError::Domain(SandboxStoreError::SequenceConflict)
    } else {
        ApplyError::Driver(error)
    }
}

async fn reconcile_lifecycle(
    store: &MongoSandboxStore,
    receipt: &LifecycleReceipt,
    snapshot_time: Timestamp,
) -> Result<bool, SandboxStoreError> {
    let mut session = store
        .client
        .start_session()
        .snapshot(true)
        .snapshot_time(snapshot_time)
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
    let sandbox = store
        .sandboxes
        .find_one(doc! { "_id": receipt.sandbox.id.as_str() })
        .read_concern(ReadConcern::snapshot())
        .session(&mut session)
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
        .as_ref()
        .map(read_sandbox)
        .transpose()?;
    if sandbox.as_ref() != Some(&receipt.sandbox) {
        return Ok(false);
    }
    for expected in &receipt.agents {
        let actual = store
            .agents
            .find_one(doc! { "_id": expected.id.as_str() })
            .read_concern(ReadConcern::snapshot())
            .session(&mut session)
            .await
            .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
            .as_ref()
            .map(read_agent)
            .transpose()?;
        if actual.as_ref() != Some(expected) {
            return Ok(false);
        }
    }
    for expected in &receipt.logs {
        let actual = store
            .agent_log
            .find_one(doc! {
                "agent_id": expected.agent_id.as_str(),
                "seq": signed(expected.seq, "reconciled lifecycle log sequence")
                    .map_err(SandboxStoreError::Corrupt)?,
            })
            .read_concern(ReadConcern::snapshot())
            .session(&mut session)
            .await
            .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
            .as_ref()
            .map(read_entry)
            .transpose()?;
        if actual.as_ref() != Some(expected) {
            return Ok(false);
        }
    }
    for expected in &receipt.events {
        if reconcile_event_with_snapshot_session(store, expected, &mut session)
            .await?
            .is_none()
        {
            return Ok(false);
        }
    }
    Ok(true)
}
