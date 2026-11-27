//! Atomic Mongo publication for one fenced agent work step.

use crate::store::agent_log_telemetry::ScanCaller;
use layer2_core::sandbox::store::{
    agent_cleanup_capacity_retry, validate_agent_cleanup_capacity_log_with_retry,
    validate_agent_work_mutation, validate_agent_work_mutation_with_sandbox, AgentWorkMutation,
    AgentWorkPublication, AgentWorkPublicationParents, SandboxStoreError,
};
use layer2_core::sandbox::{
    Agent, AgentLogEntry, AgentStatus, AgentWorkStep, Sandbox, SandboxEvent, Worktree,
};
use mongodb::bson::{doc, Bson, Document};

use super::agent_log::scan_complete_log_with_session;
use super::agent_log_integrity::{ensure_append_batch_size, prepare_append, TransactionLogError};
use super::agent_worker_capacity::publication_capacity;
use super::agents::{agent_replacement_fields, read_agent};
use super::sandbox_events::{append_in_transaction, TransactionEventError};
use super::sandbox_mongo::{
    commit_transaction, is_duplicate_key, is_transient, record_bson, signed,
    wait_for_reconciliation, CommitOutcome, MongoSandboxStore, DOCUMENT_SCHEMA_VERSION,
    TRANSACTION_ATTEMPTS,
};
use super::worktrees::{read_worktree, worktree_replacement_fields};

mod contention;
mod owner_stop;
pub(super) mod pending_adoption;
pub(super) mod restart_ready;
#[cfg(test)]
pub(crate) use pending_adoption::test_hooks as adoption_probe;
mod persistence;
mod rebuild_parents;
use persistence::{advance_agent, advance_worktree};
mod prepared_replay;
#[cfg(test)]
mod prepared_replay_tests;
mod publication_history;
mod publication_log;
mod publication_transaction;
pub(in crate::store) mod retry_audit;
pub(super) use publication_transaction::commit_connection as commit_agent_connection_work;
use publication_transaction::PublicationGuards;
mod prepared_open_ready;
mod segmented;
mod sign_in;
mod skill_result;
#[cfg(test)]
pub(crate) mod skill_result_test_support;
use super::agent_log_segments::transaction_scope::TransactionScope;
use publication_log::{final_agent_and_logs, insert_logs};
#[cfg(test)]
mod prepared_open_ready_tests;

#[cfg_attr(test, derive(Clone))]
pub(super) struct PublicationReceipt {
    pub(super) segmented: Option<super::agent_log_segments::writer_commit::Recorded>,
    pub(super) agent: Agent,
    pub(super) worktree: Option<Worktree>,
    pub(super) logs: Vec<AgentLogEntry>,
    pub(super) event: Option<SandboxEvent>,
    pub(super) sign_in_sandbox: Option<Sandbox>,
}

enum ApplyError {
    Domain(SandboxStoreError),
    Driver(mongodb::error::Error),
    PreparedIneligible,
    StaleProposal,
    StagePending(Box<pending_adoption::Source>),
    PendingReplayed,
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
    publication_transaction::commit(store, mutation, None, None, None)
        .await
        .map(|_| ())
}

pub(super) async fn commit_agent_skill_result(
    store: &MongoSandboxStore,
    mutation: AgentWorkMutation,
    guard: &layer2_core::sandbox::store::AgentSkillResultGuard,
) -> Result<(), SandboxStoreError> {
    publication_transaction::commit(store, mutation, None, None, Some(guard))
        .await
        .map(|_| ())
}

pub(super) async fn try_publish_agent_work(
    store: &MongoSandboxStore,
    mutation: AgentWorkMutation,
    parents: AgentWorkPublicationParents,
) -> Result<AgentWorkPublication, SandboxStoreError> {
    publication_transaction::try_publish(store, mutation, parents).await
}

pub(super) async fn commit_prepared_agent_open_ready(
    store: &MongoSandboxStore,
    mutation: AgentWorkMutation,
    observed: layer2_core::sandbox::store::PreparedAgentOpenContext,
) -> Result<bool, SandboxStoreError> {
    publication_transaction::commit(store, mutation, Some(&observed), None, None).await
}

#[cfg(test)]
async fn apply_publication(
    store: &MongoSandboxStore,
    mutation: &AgentWorkMutation,
    scope: TransactionScope<'_>,
) -> Result<PublicationReceipt, ApplyError> {
    apply_publication_observed(
        store,
        mutation,
        None,
        None,
        &mut publication_history::AttemptHistory::default(),
        PublicationGuards::default(),
        scope,
    )
    .await
}

pub(super) use publication_transaction::commit_repeated_failure;

async fn apply_publication_observed(
    store: &MongoSandboxStore,
    mutation: &AgentWorkMutation,
    observed: Option<&layer2_core::sandbox::store::PreparedAgentOpenContext>,
    repeated_marker: Option<&AgentLogEntry>,
    history_cache: &mut publication_history::AttemptHistory,
    guards: PublicationGuards<'_>,
    mut scope: TransactionScope<'_>,
) -> Result<PublicationReceipt, ApplyError> {
    let current_row = scope
        .find_one(&store.agents, doc! { "_id": mutation.agent.id.as_str() })
        .await
        .map_err(ApplyError::Driver)?
        .ok_or(ApplyError::Domain(SandboxStoreError::NotFound))?;
    let current = read_agent(&current_row).map_err(ApplyError::Domain)?;
    if let Some(parents) = guards.rebuild_parents {
        if current.revision != mutation.expected_agent_revision {
            return Err(ApplyError::StaleProposal);
        }
        rebuild_parents::fence(store, &current, mutation, parents, &mut scope).await?;
    }
    skill_result::fence(store, &current, mutation, guards.skill_result, &mut scope).await?;
    let format =
        super::agent_log_segments::read_format(&current_row).map_err(ApplyError::Domain)?;
    if guards.adoption.is_none() {
        pending_adoption::select(
            store,
            &current_row,
            &current,
            mutation,
            format.is_some(),
            guards
                .observed_connection
                .filter(|_| !guards.adoption_not_selected),
            &mut scope,
        )
        .await?;
    }
    let segmented = if let Some(prepared) = guards.adoption {
        Some(
            segmented::publish_adopted(
                store,
                &current_row,
                &current,
                mutation,
                prepared,
                &mut scope,
            )
            .await
            .map_err(|error| contention::phase(mutation, "segmented_adoption", error))?,
        )
    } else if format.is_some() {
        Some(
            segmented::publish(store, &current_row, &current, mutation, &mut scope)
                .await
                .map_err(|error| contention::phase(mutation, "segmented_publication", error))?,
        )
    } else {
        None
    };
    let session = scope.release();
    if super::native_retirement_action::adds_obligation(&current, &mutation.agent)
        && !super::native_retirement_action::fence_obligation(store, &current.sandbox_id, session)
            .await
            .map_err(|error| {
                if error.contains_label(mongodb::error::UNKNOWN_TRANSACTION_COMMIT_RESULT) {
                    ApplyError::Domain(SandboxStoreError::Unavailable(error.to_string()))
                } else {
                    ApplyError::Driver(error)
                }
            })?
    {
        return Err(ApplyError::Domain(SandboxStoreError::Conflict));
    }
    #[cfg(test)]
    if current.work.step == AgentWorkStep::FenceInteraction {
        use pending_adoption::test_hooks::{self, Boundary};
        test_hooks::stop(
            &store.agents.namespace().db,
            Boundary::BeforeInteractionFenceAuthority,
        )
        .await;
    }
    store
        .fence_agent_effect_transaction(&mutation.agent.id, session)
        .await
        .map_err(|error| contention::ownership_error(mutation, error))
        .map_err(|error| contention::phase(mutation, "manager_authority", error))?;
    // Decide eligibility in the publication's own snapshot. The same consumed
    // preparation revision is fenced below; no independent preflight can turn
    // a different snapshot into an unnecessary ordinary startup fallback.
    let prepared_revision = match observed {
        Some(observed) => {
            Some(prepared_open_ready::proof_revision(store, &current, observed, session).await?)
        }
        None => None,
    };
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
    let history = if segmented.is_none() {
        Some(
            Box::pin(publication_history::read(
                store,
                &current,
                &current_row,
                mutation,
                publication_history::Selection {
                    ordinary: observed.is_none(),
                    repeated: repeated_marker.is_some(),
                },
                history_cache,
                session,
            ))
            .await?,
        )
    } else {
        None
    };
    let entry_count = history
        .as_ref()
        .map_or(current.conversation.last_seq, |h| h.summary.entry_count());
    let last_log = segmented
        .as_ref()
        .and_then(|r| r.latest_before.as_ref())
        .or_else(|| history.as_ref().and_then(|h| h.last.as_ref()));
    validate_agent_cleanup_capacity_log_with_retry(&current, mutation, last_log, repeated_marker)
        .map_err(ApplyError::Domain)?;
    if current.work.step == AgentWorkStep::WaitForWorkspace
        && mutation.agent.work.step == AgentWorkStep::AddWorktree
    {
        rebuild_parents::fence_workspace_binding(store, &current, mutation, session).await?;
    }
    if let Some(observed) = observed {
        layer2_core::sandbox::store::validate_prepared_agent_open_ready(
            &current,
            &current_worktree,
            entry_count,
            mutation,
        )
        .map_err(|error| contention::validation_error(&current, mutation, error))?;
        prepared_open_ready::fence(
            store,
            &current,
            &current_worktree,
            observed,
            prepared_revision.expect("observed publication proved preparation"),
            session,
        )
        .await?;
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
        persistence::fence_retired_sandbox(
            store,
            &current,
            &current_worktree,
            entry_count,
            mutation,
            session,
        )
        .await?;
    } else {
        validate_agent_work_mutation(&current, &current_worktree, entry_count, mutation)
            .map_err(|error| contention::validation_error(&current, mutation, error))?;
    }

    owner_stop::settle(
        store,
        &current_row,
        &current,
        mutation,
        guards.rebuild_parents,
        session,
    )
    .await?;
    let sign_in_sandbox = sign_in::apply(store, &current, mutation, session).await?;
    prepared_replay::fence(store, &current, &current_worktree, mutation, session).await?;
    let (final_agent, logs) = final_agent_and_logs(&current, mutation)?;

    if let Some(history) = &history {
        let complete_log = &history.summary;
        let prior_semantic = match complete_log.semantic() {
            Some(checkpoint) => checkpoint.clone(),
            None => {
                crate::store::agent_log_integrity::audit_checkpoint(
                    store,
                    &current,
                    &history.entries,
                    session,
                )
                .await?
            }
        };
        history.audited(&current);
        let next_semantic = prior_semantic
            .after_append(&logs, &final_agent)
            .map_err(ApplyError::Domain)?;
        let prepared = prepare_append(
            &current,
            complete_log,
            &logs,
            final_agent.revision,
            next_semantic.clone(),
            publication_capacity(&current, mutation),
        );
        let integrity = match prepared {
            Err(SandboxStoreError::AgentLogCapacityExceeded)
                if super::agent_worker_capacity::stop_can_use_delete_reserve(
                    &current, mutation,
                ) =>
            {
                prepare_append(
                    &current,
                    complete_log,
                    &logs,
                    final_agent.revision,
                    next_semantic,
                    super::agent_log_integrity::AppendCapacity::Delete,
                )
            }
            result => result,
        }
        .map_err(ApplyError::Domain)?;
        advance_agent(
            store,
            &current_row,
            &current,
            &final_agent,
            mutation,
            integrity.integrity,
            session,
        )
        .await?;
        insert_logs(store, &integrity.documents, session).await?;
    }
    restart_ready::finish(
        store,
        (&current_row, &current, &final_agent),
        guards.rebuild_parents,
        session,
    )
    .await?;
    if let Some(versioned) = mutation.worktree.as_ref() {
        advance_worktree(store, &current_worktree, &versioned.worktree, session)
            .await
            .map_err(|error| contention::phase(mutation, "worktree_write", error))?;
    }
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
    if let (Some(adoption), Some(event)) = (guards.adoption, &event) {
        adoption.bind_sandbox_event(store, event, session).await?;
    }
    Ok(PublicationReceipt {
        segmented,
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
