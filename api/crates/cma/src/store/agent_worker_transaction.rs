//! Atomic Mongo publication for one fenced agent work step.

use layer2_core::sandbox::store::{
    validate_agent_work_mutation, validate_agent_work_mutation_with_sandbox, AgentWorkMutation,
    SandboxStoreError,
};
use layer2_core::sandbox::{
    Agent, AgentLogEntry, AgentStatus, AgentWorkStep, Sandbox, SandboxEvent, Worktree,
};
use mongodb::bson::{doc, Bson, Document};

use super::agent_log::{log_document, scan_complete_log_with_session};
use super::agent_log_integrity::{
    ensure_append_batch_size, ensure_append_capacity, TransactionLogError,
};
use super::agent_worker_capacity::publication_capacity;
use super::agents::{agent_replacement_fields, read_agent};
use super::sandbox_events::{append_in_transaction, TransactionEventError};
use super::sandbox_mongo::{
    commit_transaction, is_duplicate_key, is_transient, record_bson, signed, start_transaction,
    wait_for_reconciliation, CommitOutcome, MongoSandboxStore, DOCUMENT_SCHEMA_VERSION,
    TRANSACTION_ATTEMPTS,
};
use super::worktrees::{read_worktree, worktree_replacement_fields};

<<<<<<< HEAD
mod prepared_replay;
#[cfg(test)]
mod prepared_replay_tests;
=======
mod publication_log;
>>>>>>> 92c2e24f (Atomically publish prepared agent open and ready)
mod sign_in;
use publication_log::{final_agent_and_logs, insert_logs};
mod prepared_open_ready;
#[cfg(test)]
mod prepared_open_ready_tests;

#[cfg_attr(test, derive(Clone))]
pub(super) struct PublicationReceipt {
    pub(super) agent: Agent,
    pub(super) worktree: Option<Worktree>,
    pub(super) logs: Vec<AgentLogEntry>,
    pub(super) event: Option<SandboxEvent>,
    pub(super) sign_in_sandbox: Option<Sandbox>,
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

impl From<TransactionLogError> for ApplyError {
    fn from(error: TransactionLogError) -> Self {
        match error {
            TransactionLogError::Domain(error) => Self::Domain(error),
            TransactionLogError::Driver(error) => Self::Driver(error),
        }
    }
}

pub(super) async fn commit_agent_work(
    store: &MongoSandboxStore,
    mutation: AgentWorkMutation,
) -> Result<(), SandboxStoreError> {
    commit_agent_work_observed(store, mutation, None).await
}

pub(super) async fn commit_prepared_agent_open_ready(
    store: &MongoSandboxStore,
    mutation: AgentWorkMutation,
    observed: layer2_core::sandbox::store::PreparedAgentOpenContext,
) -> Result<bool, SandboxStoreError> {
    if !prepared_open_ready::eligible(store, &mutation, &observed).await? {
        return Ok(false);
    }
    commit_agent_work_observed(store, mutation, Some(&observed)).await?;
    Ok(true)
}

async fn commit_agent_work_observed(
    store: &MongoSandboxStore,
    mutation: AgentWorkMutation,
    observed: Option<&layer2_core::sandbox::store::PreparedAgentOpenContext>,
) -> Result<(), SandboxStoreError> {
    ensure_append_batch_size(mutation.agent_logs.len())?;
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
        let publication = if observed.is_some() {
            apply_publication_observed(store, &mutation, observed, &mut session).await
        } else {
            apply_publication(store, &mutation, &mut session).await
        };
        let receipt = match publication {
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
                        "agent work publication commit remained unknown without an operation time: {error}"
                    ))
                })?;
                let commit_error = error.to_string();
                let unresolved_error = commit_error.clone();
                let store = store.clone();
                return wait_for_reconciliation("agent work publication commit", async move {
                    if super::agent_worker_reconcile::reconcile_publication_after_unknown(
                        &store,
                        &receipt,
                        snapshot_time,
                    )
                    .await?
                    {
                        Ok(())
                    } else {
                        Err(SandboxStoreError::Unavailable(format!(
                            "agent work publication commit remained unknown: {unresolved_error}"
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
    apply_publication_observed(store, mutation, None, session).await
}

async fn apply_publication_observed(
    store: &MongoSandboxStore,
    mutation: &AgentWorkMutation,
    observed: Option<&layer2_core::sandbox::store::PreparedAgentOpenContext>,
    session: &mut mongodb::ClientSession,
) -> Result<PublicationReceipt, ApplyError> {
    let current_row = store
        .agents
        .find_one(doc! { "_id": mutation.agent.id.as_str() })
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?
        .ok_or(ApplyError::Domain(SandboxStoreError::NotFound))?;
    let current = read_agent(&current_row).map_err(ApplyError::Domain)?;
    let current_worktree = store
        .worktrees
        .find_one(doc! { "_id": current.worktree_id.as_str() })
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?
        .as_ref()
        .map(read_worktree)
        .transpose()
        .map_err(ApplyError::Domain)?
        .ok_or(ApplyError::Domain(SandboxStoreError::NotFound))?;
    let complete_log = scan_complete_log_with_session(
        store,
        &current.id,
        current.conversation.last_seq,
        session,
        |_| Ok(()),
    )
    .await?;
    if current.work.step == AgentWorkStep::WaitForWorkspace
        && mutation.agent.work.step == AgentWorkStep::AddWorktree
    {
        let sandbox = store
            .sandboxes
            .find_one(doc! {"_id": current.sandbox_id.as_str()})
            .session(&mut *session)
            .await
            .map_err(ApplyError::Driver)?
            .as_ref()
            .map(super::sandboxes::read_sandbox)
            .transpose()
            .map_err(ApplyError::Domain)?
            .ok_or(ApplyError::Domain(SandboxStoreError::NotFound))?;
        let workspace = store
            .workspaces
            .find_one(doc! {"_id": current.workspace_id.as_str()})
            .session(&mut *session)
            .await
            .map_err(ApplyError::Driver)?
            .as_ref()
            .map(super::workspaces::read_workspace)
            .transpose()
            .map_err(ApplyError::Domain)?
            .ok_or(ApplyError::Domain(SandboxStoreError::NotFound))?;
        layer2_core::sandbox::store::validate_agent_workspace_binding(
            &current, mutation, &sandbox, &workspace,
        )
        .map_err(ApplyError::Domain)?;
        for (collection, id, revision) in [
            (&store.sandboxes, sandbox.id.as_str(), sandbox.revision),
            (&store.workspaces, workspace.id.as_str(), workspace.revision),
        ] {
            let written = collection.update_one(doc! {"_id": id,
                "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                "revision": signed(revision, "bootstrap parent revision").map_err(|_| ApplyError::Domain(SandboxStoreError::Conflict))?,
                "owned_workspace_cleanup_admission_v1": {"$exists": false},
                "channel_reuse_cleanup_execution": {"$exists": false}},
                doc! {"$inc": {"agent_bootstrap_binding_fence": 1_i64}})
                .session(&mut *session).await.map_err(ApplyError::Driver)?;
            if written.matched_count != 1 {
                return Err(ApplyError::Domain(SandboxStoreError::Conflict));
            }
        }
    }
    if let Some(observed) = observed {
        layer2_core::sandbox::store::validate_prepared_agent_open_ready(
            &current,
            &current_worktree,
            complete_log.entry_count(),
            mutation,
        )
        .map_err(ApplyError::Domain)?;
        prepared_open_ready::fence(store, &current, &current_worktree, observed, session).await?;
    } else if (current.status == AgentStatus::Paused
        && current.work.step == AgentWorkStep::CloseProcess
        && mutation.agent.work.step == AgentWorkStep::PublishStopped
        || matches!(current.status, AgentStatus::Stopped | AgentStatus::Failed)
            && matches!(
                current.work.step,
                AgentWorkStep::EnsureProcessClosed
                    | AgentWorkStep::RemoveHome
                    | AgentWorkStep::RemoveWorktree
            ))
        && mutation.agent.work.receipts == current.work.receipts
    {
        let sandbox = store
            .sandboxes
            .find_one(doc! { "_id": current.sandbox_id.as_str() })
            .session(&mut *session)
            .await
            .map_err(ApplyError::Driver)?
            .as_ref()
            .map(super::sandboxes::read_sandbox)
            .transpose()
            .map_err(ApplyError::Domain)?
            .ok_or(ApplyError::Domain(SandboxStoreError::NotFound))?;
        validate_agent_work_mutation_with_sandbox(
            &current,
            &current_worktree,
            complete_log.entry_count(),
            mutation,
            &sandbox,
        )
        .map_err(ApplyError::Domain)?;
        // Serialize the no-runtime proof with any concurrent parent publication.
        let written = store
            .sandboxes
            .update_one(
                doc! {
                    "_id": sandbox.id.as_str(),
                    "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                    "revision": signed(sandbox.revision, "retiring sandbox revision")
                        .map_err(|_| ApplyError::Domain(SandboxStoreError::Conflict))?,
                },
                doc! { "$inc": { "runtime_writer_claim_fence": 1_i64 } },
            )
            .session(&mut *session)
            .await
            .map_err(ApplyError::Driver)?;
        if written.matched_count != 1 {
            return Err(ApplyError::Domain(SandboxStoreError::Conflict));
        }
    } else {
        validate_agent_work_mutation(
            &current,
            &current_worktree,
            complete_log.entry_count(),
            mutation,
        )
        .map_err(ApplyError::Domain)?;
    }

    let sign_in_sandbox = sign_in::apply(store, &current, mutation, session).await?;
    prepared_replay::fence(store, &current, &current_worktree, mutation, session).await?;
    let (final_agent, logs) = final_agent_and_logs(&current, mutation)?;
    ensure_append_capacity(
        &complete_log,
        &logs,
        publication_capacity(&current, mutation),
    )
    .map_err(ApplyError::Domain)?;
    advance_agent(
        store,
        &current_row,
        &current,
        &final_agent,
        mutation,
        session,
    )
    .await?;
    if let Some(versioned) = mutation.worktree.as_ref() {
        advance_worktree(store, &current_worktree, &versioned.worktree, session).await?;
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
        sign_in_sandbox,
    })
}

async fn advance_agent(
    store: &MongoSandboxStore,
    current_row: &Document,
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
    super::agent_conversation_store::carry_checkpoint_fields(
        current_row,
        current,
        next,
        &mut fields,
    )
    .map_err(ApplyError::Domain)?;
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
                "channel_reuse_cleanup_execution": { "$exists": false },
                "channel_history_seal": { "$exists": false },
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

fn classify_aggregate_write(error: mongodb::error::Error) -> ApplyError {
    if is_duplicate_key(&error) {
        ApplyError::Domain(SandboxStoreError::Duplicate)
    } else {
        ApplyError::Driver(error)
    }
}
