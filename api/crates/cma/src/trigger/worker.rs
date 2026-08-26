//! Database-led execution of one fenced Launch effect.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use layer2_core::{
    rfc3339_at, EffectClaim, EffectClaimMode, Launch, LaunchEffectResult, LaunchFailure, LaunchId,
    TriggerService, TriggerServiceError,
};

const DEFAULT_WORKER_LEASE: Duration = Duration::from_secs(120);

trait WorkerClock: Send + Sync {
    fn now(&self) -> SystemTime;
}

struct SystemWorkerClock;

impl WorkerClock for SystemWorkerClock {
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }
}

/// Result of an adapter call after its effect claim is durably committed.
#[derive(Debug, Clone)]
pub(crate) enum EffectExecution {
    Completed {
        result: LaunchEffectResult,
        safe_detail: String,
        external_resource_ids: BTreeMap<String, String>,
    },
    RetryLater {
        failure: LaunchFailure,
        retry_at: String,
    },
    ReconcileLater {
        failure: LaunchFailure,
        reconcile_at: String,
        external_resource_ids: BTreeMap<String, String>,
    },
    RetryWithProgress {
        result: LaunchEffectResult,
        failure: LaunchFailure,
        retry_at: String,
    },
    ReconcileWithProgress {
        result: LaunchEffectResult,
        failure: LaunchFailure,
        reconcile_at: String,
        external_resource_ids: BTreeMap<String, String>,
    },
    FailedWithProgress {
        result: LaunchEffectResult,
        failure: LaunchFailure,
    },
    Failed {
        failure: LaunchFailure,
    },
}

/// Provider/runtime implementation for the closed Launch effect vocabulary.
#[async_trait]
pub(crate) trait EffectExecutor: Send + Sync {
    async fn execute(&self, launch: &Launch, claim: &EffectClaim) -> EffectExecution;

    async fn reconcile(&self, launch: &Launch, claim: &EffectClaim) -> EffectExecution;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EffectRunOutcome {
    NotAcquired,
    NoWork,
    Completed,
    Deferred,
    Failed,
}

/// One in-process worker. It owns no durable queue state: every invocation
/// starts from the Launch row, and an expired process lease is reclaimable.
pub(crate) struct EffectWorker {
    triggers: Arc<TriggerService>,
    executor: Arc<dyn EffectExecutor>,
    holder: String,
    clock: Arc<dyn WorkerClock>,
    lease_duration: Duration,
}

impl EffectWorker {
    pub(crate) fn new(
        triggers: Arc<TriggerService>,
        executor: Arc<dyn EffectExecutor>,
        holder: impl Into<String>,
    ) -> Result<Self, TriggerServiceError> {
        let holder = holder.into();
        if holder.trim().is_empty() || holder.len() > 256 {
            return Err(layer2_core::TriggerValidationError::field(
                "worker.holder",
                "is not usable",
            )
            .into());
        }
        Ok(Self {
            triggers,
            executor,
            holder,
            clock: Arc::new(SystemWorkerClock),
            lease_duration: DEFAULT_WORKER_LEASE,
        })
    }

    #[cfg(test)]
    fn with_clock(
        triggers: Arc<TriggerService>,
        executor: Arc<dyn EffectExecutor>,
        holder: impl Into<String>,
        clock: Arc<dyn WorkerClock>,
    ) -> Result<Self, TriggerServiceError> {
        let mut worker = Self::new(triggers, executor, holder)?;
        worker.clock = clock;
        Ok(worker)
    }

    /// Drive one candidate through at most one provider/runtime call.
    ///
    /// The explicit timestamps make the lease and test boundary deterministic;
    /// the scheduler supplies canonical CMA UTC values for each invocation.
    pub(crate) async fn run_once(
        &self,
        id: &LaunchId,
        expected_revision: u64,
        correlation_id: &str,
    ) -> Result<EffectRunOutcome, TriggerServiceError> {
        let acquired = self.clock.now();
        let acquired_at = rfc3339_at(acquired);
        let lease_expires_at = rfc3339_at(acquired + self.lease_duration);
        let Some(mut launch) = self
            .triggers
            .acquire_worker(
                id,
                expected_revision,
                &self.holder,
                &acquired_at,
                &lease_expires_at,
            )
            .await?
        else {
            return Ok(EffectRunOutcome::NotAcquired);
        };
        let lease = launch
            .lease
            .clone()
            .expect("a successfully acquired worker owns a persisted lease");
        let claim = match launch.claim_next_effect(&lease, &acquired_at, correlation_id) {
            Ok(Some(claim)) => claim,
            Ok(None) => {
                self.triggers
                    .release_worker(&launch.id, launch.revision_number, &lease)
                    .await?;
                return Ok(EffectRunOutcome::NoWork);
            }
            Err(error) => {
                let _ = self
                    .triggers
                    .release_worker(&launch.id, launch.revision_number, &lease)
                    .await;
                return Err(error.into());
            }
        };

        // This CAS is the write-before-effect boundary. If it fails, the
        // adapter is never called. If the process dies afterward, takeover
        // observes the claim and enters reconciliation mode.
        launch = self
            .triggers
            .commit_worker(launch.revision_number, &lease, launch)
            .await?;
        let execution = match claim.mode {
            EffectClaimMode::Execute => self.executor.execute(&launch, &claim).await,
            EffectClaimMode::Reconcile => self.executor.reconcile(&launch, &claim).await,
        };
        let settled_at = rfc3339_at(self.clock.now());

        let outcome = match execution {
            EffectExecution::Completed {
                result,
                safe_detail,
                external_resource_ids,
            } => {
                launch.complete_effect(
                    &lease,
                    &claim,
                    result,
                    safe_detail,
                    external_resource_ids,
                    &settled_at,
                    correlation_id,
                )?;
                EffectRunOutcome::Completed
            }
            EffectExecution::RetryLater { failure, retry_at } => {
                launch.fail_effect(&lease, &claim, failure, Some(retry_at), &settled_at)?;
                EffectRunOutcome::Deferred
            }
            EffectExecution::ReconcileLater {
                failure,
                reconcile_at,
                external_resource_ids,
            } => {
                launch.defer_reconciliation(
                    &lease,
                    &claim,
                    failure,
                    reconcile_at,
                    external_resource_ids,
                    &settled_at,
                )?;
                EffectRunOutcome::Deferred
            }
            EffectExecution::RetryWithProgress {
                result,
                failure,
                retry_at,
            } => {
                launch.observe_effect_progress(
                    &lease,
                    &claim,
                    result,
                    &settled_at,
                    correlation_id,
                )?;
                launch.fail_effect(&lease, &claim, failure, Some(retry_at), &settled_at)?;
                EffectRunOutcome::Deferred
            }
            EffectExecution::ReconcileWithProgress {
                result,
                failure,
                reconcile_at,
                external_resource_ids,
            } => {
                launch.observe_effect_progress(
                    &lease,
                    &claim,
                    result,
                    &settled_at,
                    correlation_id,
                )?;
                launch.defer_reconciliation(
                    &lease,
                    &claim,
                    failure,
                    reconcile_at,
                    external_resource_ids,
                    &settled_at,
                )?;
                EffectRunOutcome::Deferred
            }
            EffectExecution::FailedWithProgress { result, failure } => {
                launch.observe_effect_progress(
                    &lease,
                    &claim,
                    result,
                    &settled_at,
                    correlation_id,
                )?;
                launch.fail_effect(&lease, &claim, failure, None, &settled_at)?;
                EffectRunOutcome::Failed
            }
            EffectExecution::Failed { failure } => {
                launch.fail_effect(&lease, &claim, failure, None, &settled_at)?;
                EffectRunOutcome::Failed
            }
        };

        let committed = self
            .triggers
            .commit_worker(launch.revision_number, &lease, launch)
            .await?;
        self.triggers
            .release_worker(&committed.id, committed.revision_number, &lease)
            .await?;
        Ok(outcome)
    }
}

#[cfg(test)]
#[path = "worker_tests.rs"]
pub(crate) mod tests;
