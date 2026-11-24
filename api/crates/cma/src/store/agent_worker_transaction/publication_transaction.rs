//! Commit or abort one publication snapshot, including prepared eligibility.
use super::*;
use crate::store::transaction_terminal::GuardedTerminal as _;

use crate::store::agent_capacity_recovery::observations::{cohort, Outcome, Stage};
use crate::store::agent_write_coordination::CLEANUP_RETRY_DEADLINE;
use layer2_core::sandbox::store::{AgentWorkPublication, AgentWorkPublicationParents};

#[derive(Default)]
pub(super) struct PublicationGuards<'a> {
    pub(super) rebuild_parents: Option<&'a AgentWorkPublicationParents>,
    pub(super) skill_result: Option<&'a layer2_core::sandbox::store::AgentSkillResultGuard>,
    pub(super) adoption: Option<&'a pending_adoption::Prepared>,
    pub(super) adoption_not_selected: bool,
    pub(super) observed_connection: Option<AgentWorkPublicationParents>,
}

enum Completion {
    Published,
    PreparedIneligible,
    KnownAborted,
}

#[derive(Clone, Copy)]
enum Attempts {
    KeepProposal,
    Connection(AgentWorkPublicationParents),
    Rebuild(AgentWorkPublicationParents),
}

pub(in crate::store) async fn commit_connection(
    store: &MongoSandboxStore,
    mutation: AgentWorkMutation,
    parents: AgentWorkPublicationParents,
) -> Result<(), SandboxStoreError> {
    match run(
        store,
        mutation,
        None,
        None,
        Attempts::Connection(parents),
        None,
    )
    .await?
    {
        Completion::Published => Ok(()),
        _ => Err(SandboxStoreError::Conflict),
    }
}

pub(super) async fn commit(
    store: &MongoSandboxStore,
    mutation: AgentWorkMutation,
    observed: Option<&layer2_core::sandbox::store::PreparedAgentOpenContext>,
    repeated_marker: Option<&AgentLogEntry>,
    skill_guard: Option<&layer2_core::sandbox::store::AgentSkillResultGuard>,
) -> Result<bool, SandboxStoreError> {
    match run(
        store,
        mutation,
        observed,
        repeated_marker,
        Attempts::KeepProposal,
        skill_guard,
    )
    .await?
    {
        Completion::Published => Ok(true),
        Completion::PreparedIneligible => Ok(false),
        Completion::KnownAborted => Err(SandboxStoreError::Conflict),
    }
}

pub(in crate::store) async fn commit_repeated_failure(
    store: &MongoSandboxStore,
    retry: &layer2_core::sandbox::store::AgentRepeatedFailure,
) -> Result<(), SandboxStoreError> {
    commit(
        store,
        retry.mutation.clone(),
        None,
        Some(&retry.marker),
        None,
    )
    .await
    .map(|_| ())
}

pub(super) async fn try_publish(
    store: &MongoSandboxStore,
    mutation: AgentWorkMutation,
    parents: AgentWorkPublicationParents,
) -> Result<AgentWorkPublication, SandboxStoreError> {
    match run(
        store,
        mutation,
        None,
        None,
        Attempts::Rebuild(parents),
        None,
    )
    .await?
    {
        Completion::Published => Ok(AgentWorkPublication::Published),
        Completion::KnownAborted => Ok(AgentWorkPublication::KnownAborted),
        Completion::PreparedIneligible => Err(SandboxStoreError::InvalidTransaction(
            "ordinary work publication cannot require prepared admission",
        )),
    }
}

async fn run(
    store: &MongoSandboxStore,
    mutation: AgentWorkMutation,
    observed: Option<&layer2_core::sandbox::store::PreparedAgentOpenContext>,
    repeated_marker: Option<&AgentLogEntry>,
    attempts: Attempts,
    skill_guard: Option<&layer2_core::sandbox::store::AgentSkillResultGuard>,
) -> Result<Completion, SandboxStoreError> {
    let (limit, parents, observed_connection) = match attempts {
        Attempts::KeepProposal => (TRANSACTION_ATTEMPTS, None, None),
        Attempts::Connection(parents) => (TRANSACTION_ATTEMPTS, None, Some(parents)),
        Attempts::Rebuild(parents) => (1, Some(parents), None),
    };
    ensure_append_batch_size(mutation.agent_logs.len())?;
    let mut timing = contention::PublicationTiming::new(&mutation);
    let deadline = CLEANUP_RETRY_DEADLINE.try_with(|deadline| *deadline).ok();
    let lock_writer = || async {
        match deadline {
            Some(deadline) => store
                .agent_writes
                .lock_until(&mutation.agent.id, deadline)
                .await
                .ok_or(SandboxStoreError::Conflict),
            None => Ok(store.agent_writes.lock(&mutation.agent.id).await),
        }
    };
    let mut writer = lock_writer().await?;
    timing.acquired();
    let mut history_cache = publication_history::AttemptHistory::default();
    let mut last_transient = None;
    let jitter = uuid::Uuid::new_v4().as_u128() as u64;
    let mut staged = None;
    let mut adoption_not_selected = false;
    let mut attempt = 0;
    while attempt < limit {
        timing.attempt();
        contention::backoff(attempt, jitter).await;
        attempt += 1;
        if deadline.is_some_and(|deadline| tokio::time::Instant::now() >= deadline) {
            return Err(SandboxStoreError::Conflict);
        }
        let mut session = match store.client.start_session().await {
            Ok(session) => session,
            Err(error) if positively_aborted(&error) => {
                last_transient = Some(error);
                continue;
            }
            Err(error) => return Err(SandboxStoreError::Unavailable(error.to_string())),
        };
        let scope = match TransactionScope::start(&mut session).await {
            Ok(scope) => scope,
            Err(error) => {
                if positively_aborted(&error) {
                    last_transient = Some(error);
                    continue;
                }
                return Err(SandboxStoreError::Unavailable(error.to_string()));
            }
        };
        let publication = Box::pin(apply_publication_observed(
            store,
            &mutation,
            observed,
            repeated_marker,
            &mut history_cache,
            PublicationGuards {
                rebuild_parents: parents.as_ref(),
                skill_result: skill_guard,
                adoption: staged.as_ref(),
                adoption_not_selected,
                observed_connection,
            },
            scope,
        ))
        .await;
        let receipt = match publication {
            Ok(receipt) => receipt,
            Err(ApplyError::StagePending(source)) => {
                session
                    .abort_guarded()
                    .await
                    .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
                drop(writer);
                staged = pending_adoption::prepare(store, *source).await?;
                writer = lock_writer().await?;
                adoption_not_selected = staged.is_none();
                attempt -= 1;
                continue;
            }
            Err(ApplyError::PendingReplayed) => {
                session
                    .abort_guarded()
                    .await
                    .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
                cohort(Stage::Completed, Outcome::Replay);
                return Ok(Completion::Published);
            }
            Err(ApplyError::PreparedIneligible) => {
                let _ = session.abort_guarded().await;
                return Ok(Completion::PreparedIneligible);
            }
            Err(ApplyError::StaleProposal) => {
                let _ = session.abort_guarded().await;
                return Ok(Completion::KnownAborted);
            }
            Err(ApplyError::Domain(error)) => {
                let _ = session.abort_guarded().await;
                if staged.is_some() {
                    cohort(Stage::Blocked, Outcome::Refused);
                }
                return Err(error);
            }
            Err(ApplyError::Driver(error)) if positively_aborted(&error) => {
                let _ = session.abort_guarded().await;
                last_transient = Some(error);
                continue;
            }
            Err(ApplyError::Driver(error)) => {
                let _ = session.abort_guarded().await;
                return Err(SandboxStoreError::Unavailable(error.to_string()));
            }
        };
        #[cfg(test)]
        if receipt.pending_adoption() {
            use pending_adoption::test_hooks::{self, Boundary};
            test_hooks::stop(&store.agents.namespace().db, Boundary::BeforeCommit).await;
        }
        if staged
            .as_ref()
            .is_some_and(|prepared| !prepared.lease_live())
        {
            if let Err(error) = session.abort_guarded().await {
                tracing::warn!(error_kind = ?error.kind, "Expired pending publication abort was not acknowledged");
            }
            cohort(Stage::Blocked, Outcome::KnownAbort);
            return Err(pending_adoption::witness::unavailable());
        }
        // Once commit is sent, the original transaction must settle or
        // reconcile even if the work lease expires while waiting for Mongo.
        if deadline.is_some_and(|deadline| tokio::time::Instant::now() >= deadline) {
            let _ = session.abort_guarded().await;
            return Err(SandboxStoreError::Conflict);
        }
        match commit_transaction(&mut session).await {
            CommitOutcome::Committed => {
                if receipt.pending_adoption() {
                    cohort(Stage::Completed, Outcome::Committed);
                }
                history_cache.committed(store);
                timing.committed();
                crate::telemetry::work::published(&receipt.logs, "committed");
                return Ok(Completion::Published);
            }
            CommitOutcome::Retry(error) => last_transient = Some(error),
            CommitOutcome::Unknown(error) => {
                if receipt.pending_adoption() {
                    cohort(Stage::Blocked, Outcome::Unknown);
                }
                let snapshot_time = session.operation_time().ok_or_else(|| {
                    SandboxStoreError::Unavailable(format!(
                        "agent work publication commit remained unknown without an operation time: {error}"
                    ))
                })?;
                let commit_error = error.to_string();
                let unresolved_error = commit_error.clone();
                let store = store.clone();
                return wait_for_reconciliation("agent work publication commit", async move {
                    if Box::pin(
                        super::super::agent_worker_reconcile::reconcile_publication_after_unknown(
                            &store,
                            &receipt,
                            snapshot_time,
                        ),
                    )
                    .await?
                    {
                        if receipt.pending_adoption() {
                            cohort(Stage::Completed, Outcome::Replay);
                        }
                        crate::telemetry::work::published(&receipt.logs, "reconciled");
                        Ok(Completion::Published)
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
    match attempts {
        Attempts::KeepProposal | Attempts::Connection(_) => {
            Err(contention::exhausted(&mutation, last_transient.as_ref()))
        }
        Attempts::Rebuild(_) => Ok(Completion::KnownAborted),
    }
}

fn positively_aborted(error: &mongodb::error::Error) -> bool {
    is_transient(error) && !error.contains_label(mongodb::error::UNKNOWN_TRANSACTION_COMMIT_RESULT)
}
