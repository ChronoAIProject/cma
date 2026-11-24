//! Atomic Mongo publication for one sandbox lifecycle phase.

use crate::store::agent_log_telemetry::ScanCaller;
use crate::store::transaction_terminal::GuardedTerminal as _;
use std::collections::BTreeMap;
mod checkpoint_receipt;
mod integrity_write;
pub(super) mod reconciliation;
mod reopen_refusal;
mod segmented;
use integrity_write::{advance_agent, classify_aggregate_write, classify_log_write};

use layer2_core::sandbox::store::{
    validate_lifecycle_mutation, ReopenRefusalFence, SandboxLifecycleAgentMutation,
    SandboxLifecycleMutation, SandboxStoreError,
};
use layer2_core::sandbox::{Agent, AgentLogEntry, Sandbox, SandboxEvent};
use mongodb::bson::{doc, Bson, Timestamp};
use mongodb::options::ReadConcern;

use super::agent_log::{read_entry, scan_complete_log_with_session};
use super::agent_log_integrity::{prepare_append, AppendCapacity, TransactionLogError};
use super::agent_log_segments::transaction_scope::TransactionScope;
use super::agents::{agent_replacement_fields, read_agent};
use super::lifecycle_execution::LifecycleLoadError;
use super::sandbox_events::{
    append_in_transaction, reconcile_event_with_snapshot_session, TransactionEventError,
};
use super::sandbox_mongo::{
    commit_transaction, is_duplicate_key, is_transient, signed, wait_for_reconciliation,
    CommitOutcome, MongoSandboxStore, DOCUMENT_SCHEMA_VERSION, TRANSACTION_ATTEMPTS,
};
use super::sandboxes::{read_sandbox, sandbox_replacement_fields};

#[derive(Clone)]
pub(super) struct LifecycleReceipt {
    pub(super) segmented: Vec<super::agent_log_segments::writer_commit::Recorded>,
    pub(super) sandbox: Sandbox,
    pub(super) agents: Vec<Agent>,
    pub(super) logs: Vec<AgentLogEntry>,
    pub(super) events: Vec<SandboxEvent>,
    pub(super) checkpoint_children: Option<checkpoint_receipt::Children>,
}

#[derive(Debug)]
pub(super) enum ApplyError {
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
    refusal: Option<ReopenRefusalFence>,
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
        let scope = match TransactionScope::start(&mut session).await {
            Ok(scope) => scope,
            Err(error) if is_transient(&error) => {
                last_transient = Some(error.to_string());
                continue;
            }
            Err(error) => return Err(SandboxStoreError::Unavailable(error.to_string())),
        };
        let applied = apply_lifecycle(store, &mutation, scope, refusal.as_ref()).await;
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
                    if reconciliation::after_unknown(&store, &receipt, snapshot_time).await? {
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

pub(super) async fn apply_lifecycle(
    store: &MongoSandboxStore,
    mutation: &SandboxLifecycleMutation,
    mut scope: TransactionScope<'_>,
    refusal: Option<&ReopenRefusalFence>,
) -> Result<LifecycleReceipt, ApplyError> {
    scope
        .fence_ordinary_work(store)
        .await
        .map_err(|error| match error {
            super::work_publication::FenceError::Domain(error) => ApplyError::Domain(error),
            super::work_publication::FenceError::Driver(error) => ApplyError::Driver(error),
        })?;
    let current = load_sandbox(store, &mutation.guard.sandbox_id, &mut scope).await?;
    let agents =
        super::agent_capacity_recovery::lineage::agents(store, &current.id, &mut scope).await?;
    let segmented = scope
        .find_one(
            &store.agents,
            doc! {
                "sandbox_id": current.id.as_str(),
                "$or": [{"document_schema_version": {"$ne": DOCUMENT_SCHEMA_VERSION}},
                    {"agent_log_format": {"$exists": true}}],
            },
        )
        .await
        .map_err(ApplyError::Driver)?
        .is_some();
    if segmented {
        super::checkpoint_capture::validate_and_fence_begin_scoped(
            store,
            &current,
            &mutation.sandbox,
            &agents,
            &mut scope,
        )
        .await?;
    }
    let checkpoint_children = if segmented {
        checkpoint_receipt::capture(store, &current, &mutation.sandbox, &agents, &mut scope).await?
    } else {
        None
    };
    // Checkpoint proofs remain inside the exclusive scope. The completed
    // checkpoint fence and all later domain writes still share this transaction;
    // any later validation/write error is aborted by the root above.
    let segment_receipts = segmented::publish(store, &agents, mutation, &mut scope).await?;
    let session = scope.release();
    if let Some(refusal) = refusal {
        reopen_refusal::fence(store, &current, refusal, session).await?;
    }
    if !segmented {
        super::checkpoint_capture::validate_and_fence_begin(
            store,
            &current,
            &mutation.sandbox,
            &agents,
            session,
        )
        .await?;
    }
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
    let mut encoded_logs = Vec::new();
    let mut integrity_by_id = BTreeMap::new();
    for change in &mutation.agents {
        let stored = by_id
            .get(&change.agent.id)
            .copied()
            .ok_or(ApplyError::Domain(SandboxStoreError::NotFound))?;
        if let Some(recorded) = segment_receipts.get(&stored.id) {
            logs.extend(recorded.logs.iter().cloned());
            continue;
        }
        let mut prior_entries = Vec::new();
        let complete = scan_complete_log_with_session(
            ScanCaller::CommandAdmission,
            store,
            &stored.id,
            stored.conversation.last_seq,
            session,
            |entry| {
                prior_entries.push(entry);
                Ok(())
            },
        )
        .await?;
        let complete = complete.with_semantic(
            crate::store::agent_log_integrity::audit_checkpoint(
                store,
                stored,
                &prior_entries,
                session,
            )
            .await?,
        );
        let batch = materialize_logs(stored, change, &mutation.appended_at)?;

        let row = rows
            .iter_mut()
            .find(|(agent, _)| agent.id == stored.id)
            .expect("loaded lifecycle agent has a validation row");
        row.1 = complete.entry_count();
        let semantic =
            complete
                .semantic()
                .cloned()
                .ok_or(ApplyError::Domain(SandboxStoreError::Corrupt(
                    "lifecycle scan did not produce semantic checkpoint".into(),
                )))?;
        let next_semantic = semantic
            .after_append(&batch, &change.agent)
            .map_err(ApplyError::Domain)?;
        integrity_by_id.insert(
            stored.id.clone(),
            prepare_append(
                stored,
                &complete,
                &batch,
                change.agent.revision,
                next_semantic,
                AppendCapacity::StopOrFailure,
            )
            .map_err(ApplyError::Domain)?,
        );
        logs.extend(batch);
    }
    validate_lifecycle_mutation(&current, &rows, mutation).map_err(ApplyError::Domain)?;
    store
        .fence_reopened_owners(mutation, session)
        .await
        .map_err(|error| match error {
            crate::store::agent_ownership::FenceError::Domain(error) => ApplyError::Domain(error),
            crate::store::agent_ownership::FenceError::Driver(error) => ApplyError::Driver(error),
        })?;

    advance_sandbox(store, &current, &mutation.sandbox, session).await?;
    for change in &mutation.agents {
        let stored = by_id
            .get(&change.agent.id)
            .copied()
            .expect("validated lifecycle agent exists");
        if segment_receipts.contains_key(&stored.id) {
            continue;
        }
        let integrity = integrity_by_id
            .remove(&stored.id)
            .ok_or(ApplyError::Domain(SandboxStoreError::Corrupt(
                "lifecycle integrity receipt missing".into(),
            )))?;
        encoded_logs.extend(integrity.documents);
        advance_agent(store, stored, change, integrity.integrity, session).await?;
    }
    for entry in &encoded_logs {
        store
            .agent_log
            .clone_with_type::<mongodb::bson::RawDocumentBuf>()
            .insert_one(entry)
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
        segmented: segment_receipts.into_values().collect(),
        sandbox: mutation.sandbox.clone(),
        agents: mutation
            .agents
            .iter()
            .map(|change| change.agent.clone())
            .collect(),
        logs,
        events,
        checkpoint_children,
    })
}

async fn load_sandbox(
    store: &MongoSandboxStore,
    id: &layer2_core::SandboxId,
    scope: &mut TransactionScope<'_>,
) -> Result<Sandbox, ApplyError> {
    scope
        .find_one(&store.sandboxes, doc! { "_id": id.as_str() })
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
    let mut filter = doc! {
        "_id": current.id.as_str(),
        "document_schema_version": DOCUMENT_SCHEMA_VERSION,
        "channel_reuse_cleanup_execution": { "$exists": false },
        "revision": signed(current.revision, "sandbox lifecycle revision")
            .map_err(|_| ApplyError::Domain(SandboxStoreError::Conflict))?,
    };
    if current.admitted_retained_runtime_transition().is_some() {
        // A retained transition must hold its original, unexpired claim even
        // when renewable work is disabled on this worker.
        let claim = crate::store::work_claim_authority::sandbox_claim(current)
            .map_err(ApplyError::Domain)?;
        filter = doc! {"$and":[filter, claim]};
    }
    let advanced = store
        .sandboxes
        .update_one(
            crate::store::work_schema::query(
                crate::store::work_publication::sandbox(filter, current.id.as_str())
                    .map_err(ApplyError::Domain)?,
            ),
            crate::store::work_lease_update::replacement(
                sandbox_replacement_fields(next).map_err(ApplyError::Domain)?,
            ),
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
