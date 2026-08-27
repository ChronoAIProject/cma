//! Database-led execution of one fenced Launch effect.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use layer2_core::{
    rfc3339_at, ActivityAction, ActivityOutcome, ActivityRecord, ActivityService, ActivityTarget,
    DetailKey, EffectClaim, EffectClaimMode, EffectCompletion, Launch, LaunchEffectKind,
    LaunchEffectResult, LaunchFailure, LaunchId, TriggerService, TriggerServiceError,
};

const DEFAULT_WORKER_LEASE: Duration = Duration::from_secs(120);
const ROLLOUT_RESUMED_REASON: &str = "rollout_permitted";

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
    /// A closed intermediate state was durably proven, but a later worker
    /// lease must perform the next external boundary for the same intent.
    Progressed {
        result: LaunchEffectResult,
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
    Paused,
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
    activity: Option<Arc<ActivityService>>,
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
            activity: None,
        })
    }

    pub(crate) fn with_activity(mut self, activity: Arc<ActivityService>) -> Self {
        self.activity = Some(activity);
        self
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
        let rollout = self.triggers.rollout_policy();
        if let Err(denial) = rollout.launch_orchestration(&launch) {
            if launch.blocked_reason.is_none() {
                launch.block(
                    layer2_core::LaunchBlockedReason::FeaturePaused,
                    acquired_at.clone(),
                    correlation_id,
                )?;
                let committed = self
                    .triggers
                    .commit_worker(launch.revision_number, &lease, launch)
                    .await?;
                let released = self
                    .triggers
                    .release_worker(&committed.id, committed.revision_number, &lease)
                    .await?;
                self.record_rollout_pause(&released, correlation_id, denial.code())
                    .await;
            } else {
                self.triggers
                    .release_worker(&launch.id, launch.revision_number, &lease)
                    .await?;
            }
            return Ok(EffectRunOutcome::Paused);
        }
        if launch.resume_feature_pause(acquired_at.clone(), correlation_id)? {
            launch = self
                .triggers
                .commit_worker(launch.revision_number, &lease, launch)
                .await?;
            self.record_rollout_resume(&launch, correlation_id).await;
        }
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

        let (outcome, audit_outcome) = match execution {
            EffectExecution::Completed {
                result,
                safe_detail,
                external_resource_ids,
            } => {
                launch.complete_effect(
                    &lease,
                    &claim,
                    EffectCompletion {
                        result,
                        safe_detail,
                        external_resource_ids,
                        now: settled_at.clone(),
                        correlation_id: correlation_id.into(),
                    },
                )?;
                (EffectRunOutcome::Completed, ActivityOutcome::Succeeded)
            }
            EffectExecution::Progressed { result } => {
                launch.observe_effect_progress(
                    &lease,
                    &claim,
                    result,
                    &settled_at,
                    correlation_id,
                )?;
                (EffectRunOutcome::Deferred, ActivityOutcome::Succeeded)
            }
            EffectExecution::RetryLater { failure, retry_at } => {
                launch.fail_effect(&lease, &claim, failure, Some(retry_at), &settled_at)?;
                (EffectRunOutcome::Deferred, ActivityOutcome::Failed)
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
                (EffectRunOutcome::Deferred, ActivityOutcome::Failed)
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
                (EffectRunOutcome::Deferred, ActivityOutcome::Failed)
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
                (EffectRunOutcome::Deferred, ActivityOutcome::Failed)
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
                (EffectRunOutcome::Failed, ActivityOutcome::Failed)
            }
            EffectExecution::Failed { failure } => {
                launch.fail_effect(&lease, &claim, failure, None, &settled_at)?;
                (EffectRunOutcome::Failed, ActivityOutcome::Failed)
            }
        };

        let committed = self
            .triggers
            .commit_worker(launch.revision_number, &lease, launch)
            .await?;
        let released = self
            .triggers
            .release_worker(&committed.id, committed.revision_number, &lease)
            .await?;
        self.record_effect(&released, &claim, correlation_id, audit_outcome)
            .await;
        Ok(outcome)
    }

    async fn record_rollout_pause(&self, launch: &Launch, correlation_id: &str, reason: &str) {
        tracing::info!(
            launch_id = %launch.id.0,
            correlation_id,
            rollout_reason = reason,
            "Trigger effect worker paused at a durable boundary"
        );
        if let Some(activity) = self.activity.as_ref() {
            activity
                .record(
                    worker_record(
                        launch,
                        ActivityAction::TriggerOrchestrationPause,
                        ActivityOutcome::Denied,
                        correlation_id,
                    )
                    .with(DetailKey::RolloutReason, reason),
                )
                .await;
        }
    }

    async fn record_rollout_resume(&self, launch: &Launch, correlation_id: &str) {
        tracing::info!(
            launch_id = %launch.id.0,
            correlation_id,
            rollout_reason = ROLLOUT_RESUMED_REASON,
            "Trigger effect worker resumed from a durable rollout pause"
        );
        if let Some(activity) = self.activity.as_ref() {
            activity
                .record(
                    worker_record(
                        launch,
                        ActivityAction::TriggerOrchestrationResume,
                        ActivityOutcome::Succeeded,
                        correlation_id,
                    )
                    .with(DetailKey::RolloutReason, ROLLOUT_RESUMED_REASON),
                )
                .await;
        }
    }

    async fn record_effect(
        &self,
        launch: &Launch,
        claim: &EffectClaim,
        correlation_id: &str,
        outcome: ActivityOutcome,
    ) {
        let action = match (claim.kind, claim.mode) {
            (LaunchEffectKind::Cleanup, _) => ActivityAction::TriggerCleanupExecute,
            (_, EffectClaimMode::Execute) => ActivityAction::TriggerEffectExecute,
            (_, EffectClaimMode::Reconcile) => ActivityAction::TriggerEffectReconcile,
        };
        if let Some(activity) = self.activity.as_ref() {
            let mut record = worker_record(launch, action, outcome, correlation_id)
                .with(DetailKey::Effect, effect_code(claim.kind))
                .with(DetailKey::FencingToken, claim.fence.to_string());
            if let Some(provider_request_id) = launch
                .effects
                .iter()
                .find(|effect| effect.intent_key == claim.intent_key)
                .and_then(|effect| effect.provider_request_id.as_ref())
            {
                record = record.with(DetailKey::ProviderRequestId, provider_request_id);
            }
            activity.record(record).await;
        }
        tracing::info!(
            launch_id = %launch.id.0,
            correlation_id,
            effect = effect_code(claim.kind),
            mode = ?claim.mode,
            fence = claim.fence,
            outcome = ?outcome,
            "Trigger effect worker settled one durable boundary"
        );
    }
}

pub(super) fn worker_record(
    launch: &Launch,
    action: ActivityAction,
    outcome: ActivityOutcome,
    correlation_id: &str,
) -> ActivityRecord {
    let mut record = ActivityRecord::new(action, outcome)
        .on(ActivityTarget::Launch(launch.id.clone()))
        .with(DetailKey::Subject, launch.owner.0.clone())
        .with(DetailKey::CorrelationId, correlation_id)
        .with(DetailKey::RevisionDigest, launch.revision_digest.clone())
        .with(
            DetailKey::RepositoryId,
            launch
                .revision
                .content
                .upstream
                .repository
                .repository_id
                .to_string(),
        )
        .with(DetailKey::LaunchState, wire_code(launch.state));
    if let Some(fork) = launch.fork.as_ref() {
        record = record.with(
            DetailKey::ForkRepositoryId,
            fork.repository.repository_id.to_string(),
        );
    }
    if let Some(connection) = launch.connection.as_ref() {
        record = record
            .with(
                DetailKey::GithubAccountId,
                connection.account.account_id.to_string(),
            )
            .with(DetailKey::ServiceId, connection.service_id.clone());
    }
    record
}

fn wire_code(value: impl serde::Serialize) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_else(|| "unknown".into())
}

fn effect_code(kind: LaunchEffectKind) -> &'static str {
    match kind {
        LaunchEffectKind::Preflight => "preflight",
        LaunchEffectKind::Fork => "fork",
        LaunchEffectKind::Branch => "branch",
        LaunchEffectKind::WorkspaceImport => "workspace_import",
        LaunchEffectKind::Sandbox => "sandbox",
        LaunchEffectKind::Session => "session",
        LaunchEffectKind::Reach => "reach",
        LaunchEffectKind::InitialDispatch => "initial_dispatch",
        LaunchEffectKind::Checkpoint => "checkpoint",
        LaunchEffectKind::Diff => "diff",
        LaunchEffectKind::BranchPublication => "branch_publication",
        LaunchEffectKind::PullRequest => "pull_request",
        LaunchEffectKind::Cleanup => "cleanup",
    }
}

#[cfg(test)]
#[path = "worker_tests.rs"]
pub(crate) mod tests;
