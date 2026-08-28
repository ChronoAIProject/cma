//! Database-led execution of one fenced Launch effect.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use layer2_core::{
    effect_requires_authority_admission, rfc3339_at, ActivityAction, ActivityOutcome,
    ActivityRecord, ActivityService, ActivityTarget, AuthorityAdmissionSnapshot,
    ConfirmationAction, DetailKey, EffectClaim, EffectClaimMode, EffectCompletion, FailureClass,
    Launch, LaunchEffectKind, LaunchEffectResult, LaunchFailure, LaunchId, PromptDeliveryState,
    TriggerService, TriggerServiceError,
};

use super::observability::{record_activity, TriggerAlert, OBSERVABILITY_SCHEMA_VERSION};
use super::pull_request_status::PullRequestStatusDisposition;

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
        external_resource_ids: BTreeMap<String, String>,
    },
    RetryLater {
        failure: LaunchFailure,
        retry_at: String,
        external_resource_ids: BTreeMap<String, String>,
    },
    ReconcileLater {
        failure: LaunchFailure,
        reconcile_at: String,
        external_resource_ids: BTreeMap<String, String>,
    },
    /// A deterministic session exists but is still provisioning, rotating, or
    /// recovering. This is observable waiting, not a failed reconciliation.
    WaitLater {
        wait_at: String,
        safe_detail: String,
        remediation: String,
        external_resource_ids: BTreeMap<String, String>,
    },
    RetryWithProgress {
        result: LaunchEffectResult,
        failure: LaunchFailure,
        retry_at: String,
        external_resource_ids: BTreeMap<String, String>,
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
    /// Refuse a fresh effect before any provider/runtime admission read or
    /// write. Reconciliation is deliberately excluded: a retained ambiguous
    /// effect must still be observed to reduce uncertainty and cleanup debt.
    fn refuse_execute(
        &self,
        _launch: &Launch,
        _claim: &EffectClaim,
        _correlation_id: &str,
    ) -> Option<LaunchFailure> {
        None
    }

    /// Perform only reads and return the exact credential-free authority that
    /// must be committed before this claim may execute a provider write.
    async fn admit(
        &self,
        _launch: &Launch,
        _claim: &EffectClaim,
    ) -> Result<Option<AuthorityAdmissionSnapshot>, EffectExecution> {
        Ok(None)
    }

    async fn execute(&self, launch: &Launch, claim: &EffectClaim) -> EffectExecution;

    async fn reconcile(&self, launch: &Launch, claim: &EffectClaim) -> EffectExecution;

    /// Perform one read-only refresh of an already-linked provider pull
    /// request. Implementations must not create, update, reopen, or otherwise
    /// mutate the provider object from this boundary.
    async fn refresh_pull_request_status(
        &self,
        _launch: &Launch,
        correlation_id: &str,
    ) -> PullRequestStatusDisposition {
        PullRequestStatusDisposition::FailLaunch {
            failure: LaunchFailure {
                class: FailureClass::TerminalSecurity,
                code: "pull_request_status_adapter_unavailable".into(),
                detail: "The configured worker cannot refresh the linked pull request.".into(),
                remediation: "Contact support; CMA retained the pull-request link and performed no provider write."
                    .into(),
                correlation_id: correlation_id.into(),
                retry_at: None,
                observed_at: rfc3339_at(SystemTime::now()),
            },
            provider_request_id: None,
        }
    }
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
        // Keep Launch and effect-settlement state off the bounded stack of
        // callers such as scheduler tasks and Tokio's test workers.
        Box::pin(self.run_once_inner(id, expected_revision, correlation_id)).await
    }

    #[inline(never)]
    async fn run_once_inner(
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
        if launch.state == layer2_core::LaunchState::PrOpen {
            // Status polling is fresh provider I/O, not retained-effect
            // reconciliation. Historical and malformed routes release their
            // lease without reaching the adapter.
            if !super::has_current_trigger_runtime_admission(&launch) {
                self.triggers
                    .release_worker(&launch.id, launch.revision_number, &lease)
                    .await?;
                return Ok(EffectRunOutcome::NoWork);
            }
            if launch.pull_request_status_due(&acquired_at)? {
                return self
                    .run_pull_request_status(launch, lease, correlation_id)
                    .await;
            }
            self.triggers
                .release_worker(&launch.id, launch.revision_number, &lease)
                .await?;
            return Ok(EffectRunOutcome::NoWork);
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

        if automatic_duplicate_dispatch(&launch, &claim) {
            let alert = TriggerAlert::DuplicateDispatch;
            let delivery = launch
                .delivery
                .as_ref()
                .expect("a duplicate-dispatch predicate requires delivery evidence");
            tracing::error!(
                event_name = "cma_trigger_alert_candidate",
                schema_version = OBSERVABILITY_SCHEMA_VERSION,
                alert_id = alert.id(),
                severity = alert.severity(),
                launch_id = %launch.id.0,
                correlation_id,
                effect = effect_code(claim.kind),
                mode = %wire_code(claim.mode),
                payload_hash = delivery.payload_hash,
                recovery_generation = delivery.recovery_generation,
                "Trigger automatic duplicate-dispatch alert candidate observed"
            );
            let _ = self
                .triggers
                .release_worker(&launch.id, launch.revision_number, &lease)
                .await;
            return Err(layer2_core::TriggerValidationError::field(
                "initial_prompt.dispatch",
                "an automatic replacement dispatch lacks an explicit owner recovery confirmation",
            )
            .into());
        }

        // This CAS is the write-before-effect boundary. If it fails, the
        // adapter is never called. If the process dies afterward, takeover
        // observes the claim and enters reconciliation mode.
        launch = self
            .triggers
            .commit_worker(launch.revision_number, &lease, launch)
            .await?;
        let execution = if claim.mode == EffectClaimMode::Execute {
            if let Some(failure) = self
                .executor
                .refuse_execute(&launch, &claim, correlation_id)
            {
                EffectExecution::Failed { failure }
            } else if effect_requires_authority_admission(claim.kind) {
                match self.executor.admit(&launch, &claim).await {
                    Ok(Some(admission)) => {
                        let admitted_at = rfc3339_at(self.clock.now());
                        launch.record_effect_authority_admission(
                            &lease,
                            &claim,
                            admission,
                            admitted_at,
                        )?;
                        // This is the authority write-before-effect CAS. A CAS
                        // failure or process death before it completes means no
                        // adapter write can have been admitted.
                        launch = self
                            .triggers
                            .commit_worker(launch.revision_number, &lease, launch)
                            .await?;
                        self.executor.execute(&launch, &claim).await
                    }
                    Ok(None) => EffectExecution::Failed {
                        failure: missing_admission_failure(&claim, &acquired_at, correlation_id),
                    },
                    Err(execution) => execution,
                }
            } else {
                self.executor.execute(&launch, &claim).await
            }
        } else {
            self.executor.reconcile(&launch, &claim).await
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
            EffectExecution::Progressed {
                result,
                external_resource_ids,
            } => {
                launch.observe_effect_progress(
                    &lease,
                    &claim,
                    result,
                    external_resource_ids,
                    &settled_at,
                    correlation_id,
                )?;
                (EffectRunOutcome::Deferred, ActivityOutcome::Succeeded)
            }
            EffectExecution::RetryLater {
                failure,
                retry_at,
                external_resource_ids,
            } => {
                launch.fail_effect(
                    &lease,
                    &claim,
                    failure,
                    external_resource_ids,
                    Some(retry_at),
                    &settled_at,
                )?;
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
            EffectExecution::WaitLater {
                wait_at,
                safe_detail,
                remediation,
                external_resource_ids,
            } => {
                launch.defer_waiting(
                    &lease,
                    &claim,
                    wait_at,
                    safe_detail,
                    remediation,
                    external_resource_ids,
                    &settled_at,
                    correlation_id,
                )?;
                (EffectRunOutcome::Deferred, ActivityOutcome::Succeeded)
            }
            EffectExecution::RetryWithProgress {
                result,
                failure,
                retry_at,
                external_resource_ids,
            } => {
                launch.observe_effect_progress(
                    &lease,
                    &claim,
                    result,
                    external_resource_ids.clone(),
                    &settled_at,
                    correlation_id,
                )?;
                launch.fail_effect(
                    &lease,
                    &claim,
                    failure,
                    external_resource_ids,
                    Some(retry_at),
                    &settled_at,
                )?;
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
                    external_resource_ids.clone(),
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
                    BTreeMap::new(),
                    &settled_at,
                    correlation_id,
                )?;
                launch.fail_effect(&lease, &claim, failure, BTreeMap::new(), None, &settled_at)?;
                (EffectRunOutcome::Failed, ActivityOutcome::Failed)
            }
            EffectExecution::Failed { failure } => {
                launch.fail_effect(&lease, &claim, failure, BTreeMap::new(), None, &settled_at)?;
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

    async fn run_pull_request_status(
        &self,
        mut launch: Launch,
        lease: layer2_core::WorkerLease,
        correlation_id: &str,
    ) -> Result<EffectRunOutcome, TriggerServiceError> {
        let disposition = self
            .executor
            .refresh_pull_request_status(&launch, correlation_id)
            .await;
        let settled_at = rfc3339_at(self.clock.now());
        let (outcome, audit_outcome) = match disposition {
            PullRequestStatusDisposition::Observed {
                pull_request,
                next_attempt_at,
                provider_request_id,
            } => {
                launch.observe_pull_request_status(
                    &lease,
                    *pull_request,
                    next_attempt_at,
                    provider_request_id,
                    &settled_at,
                    correlation_id,
                )?;
                (EffectRunOutcome::Completed, ActivityOutcome::Succeeded)
            }
            PullRequestStatusDisposition::Retry {
                failure,
                next_attempt_at,
                provider_request_id,
            } => {
                launch.defer_pull_request_status(
                    &lease,
                    failure,
                    Some(next_attempt_at),
                    provider_request_id,
                    &settled_at,
                )?;
                (EffectRunOutcome::Deferred, ActivityOutcome::Failed)
            }
            PullRequestStatusDisposition::Suspended {
                failure,
                provider_request_id,
            } => {
                launch.defer_pull_request_status(
                    &lease,
                    failure,
                    None,
                    provider_request_id,
                    &settled_at,
                )?;
                (EffectRunOutcome::Deferred, ActivityOutcome::Failed)
            }
            PullRequestStatusDisposition::FailLaunch {
                failure,
                provider_request_id,
            } => {
                let launch_failure = failure.clone();
                launch.defer_pull_request_status(
                    &lease,
                    failure,
                    None,
                    provider_request_id,
                    &settled_at,
                )?;
                launch.fail(launch_failure, &settled_at)?;
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
        self.record_pull_request_status(&released, correlation_id, audit_outcome)
            .await;
        Ok(outcome)
    }

    async fn record_pull_request_status(
        &self,
        launch: &Launch,
        correlation_id: &str,
        outcome: ActivityOutcome,
    ) {
        let record = launch.pull_request.as_ref();
        let failure = record.and_then(|record| record.status_sync.last_failure.as_ref());
        tracing::info!(
            event_name = "cma_trigger_pull_request_status_settlement",
            schema_version = OBSERVABILITY_SCHEMA_VERSION,
            launch_id = %launch.id.0,
            correlation_id,
            pull_request_state = record.map(|record| wire_code(record.state)).unwrap_or_else(|| "missing".into()),
            mergeability = record.map(|record| wire_code(record.mergeability)).unwrap_or_else(|| "unknown".into()),
            checks = record.map(|record| wire_code(record.checks)).unwrap_or_else(|| "unknown".into()),
            failure_class = failure.map(|failure| wire_code(failure.class)).unwrap_or_else(|| "none".into()),
            failure_code = failure.map_or("none", |failure| failure.code.as_str()),
            outcome = %wire_code(outcome),
            "Trigger pull-request status worker settled one read-only provider observation"
        );
        if let Some(activity) = self.activity.as_ref() {
            let mut activity_record = worker_record(
                launch,
                ActivityAction::TriggerEffectReconcile,
                outcome,
                correlation_id,
            )
            .with(DetailKey::Effect, "pull_request_status");
            if let Some(provider_request_id) =
                record.and_then(|record| record.status_sync.provider_request_id.as_ref())
            {
                activity_record =
                    activity_record.with(DetailKey::ProviderRequestId, provider_request_id.clone());
            }
            record_activity(activity, activity_record).await;
        }
    }

    async fn record_rollout_pause(&self, launch: &Launch, correlation_id: &str, reason: &str) {
        tracing::info!(
            launch_id = %launch.id.0,
            correlation_id,
            rollout_reason = reason,
            "Trigger effect worker paused at a durable boundary"
        );
        if let Some(activity) = self.activity.as_ref() {
            record_activity(
                activity,
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
            record_activity(
                activity,
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
        let effect = launch
            .effects
            .iter()
            .find(|effect| effect.intent_key == claim.intent_key);
        if let Some(activity) = self.activity.as_ref() {
            let mut record = worker_record(launch, action, outcome, correlation_id)
                .with(DetailKey::Effect, effect_code(claim.kind))
                .with(DetailKey::FencingToken, claim.fence.to_string());
            if let Some(provider_request_id) =
                effect.and_then(|effect| effect.provider_request_id.as_ref())
            {
                record = record.with(DetailKey::ProviderRequestId, provider_request_id);
            }
            record_activity(activity, record).await;
        }
        let failure = effect.and_then(|effect| effect.last_failure.as_ref());
        let failure_class = failure
            .map(|failure| wire_code(failure.class))
            .unwrap_or_else(|| "none".into());
        let failure_code = failure.map_or("none", |failure| failure.code.as_str());
        let effect_state = effect
            .map(|effect| wire_code(effect.state))
            .unwrap_or_else(|| "unknown".into());
        tracing::info!(
            event_name = "cma_trigger_effect_settlement",
            schema_version = OBSERVABILITY_SCHEMA_VERSION,
            launch_id = %launch.id.0,
            correlation_id,
            effect = effect_code(claim.kind),
            mode = %wire_code(claim.mode),
            fence = claim.fence,
            outcome = %wire_code(outcome),
            effect_state,
            failure_class,
            failure_code,
            "Trigger effect worker settled one durable boundary"
        );
        if provider_degraded_failure_code(failure_code) {
            let alert = TriggerAlert::ProviderDegraded;
            tracing::warn!(
                event_name = "cma_trigger_alert_candidate",
                schema_version = OBSERVABILITY_SCHEMA_VERSION,
                alert_id = alert.id(),
                severity = alert.severity(),
                launch_id = %launch.id.0,
                correlation_id,
                effect = effect_code(claim.kind),
                failure_code,
                "Trigger provider degradation alert candidate observed"
            );
        }
        if unauthorized_effect_candidate(failure) {
            let alert = TriggerAlert::UnauthorizedEffect;
            tracing::error!(
                event_name = "cma_trigger_alert_candidate",
                schema_version = OBSERVABILITY_SCHEMA_VERSION,
                alert_id = alert.id(),
                severity = alert.severity(),
                launch_id = %launch.id.0,
                correlation_id,
                effect = effect_code(claim.kind),
                mode = %wire_code(claim.mode),
                fence = claim.fence,
                failure_code,
                "Trigger unauthorized-effect alert candidate observed"
            );
        }
    }
}

fn missing_admission_failure(
    claim: &EffectClaim,
    observed_at: &str,
    correlation_id: &str,
) -> LaunchFailure {
    LaunchFailure {
        class: FailureClass::TerminalSecurity,
        code: "trigger_authority_admission_missing".into(),
        detail: "The effect adapter did not produce a durable authority admission.".into(),
        remediation: "Contact support; CMA did not perform the external write.".into(),
        correlation_id: if correlation_id.trim().is_empty() {
            format!("{}:admission", claim.intent_key)
        } else {
            correlation_id.into()
        },
        retry_at: None,
        observed_at: observed_at.into(),
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

fn provider_degraded_failure_code(code: &str) -> bool {
    matches!(
        code,
        "github_rate_limited"
            | "github_temporarily_unavailable"
            | "github_provider_authentication_unavailable"
            | "trigger_runtime_broker_temporarily_unavailable"
    )
}

fn unauthorized_effect_candidate(failure: Option<&LaunchFailure>) -> bool {
    failure.is_some_and(|failure| failure.class == FailureClass::TerminalSecurity)
}

/// Detect the invariant breach that would turn an ambiguous initial task into
/// an automatic second owner turn. An explicit recovery confirmation grants
/// exactly the next generation; a worker claim without it is stopped before
/// the claim CAS or adapter boundary.
fn automatic_duplicate_dispatch(launch: &Launch, claim: &EffectClaim) -> bool {
    if claim.kind != LaunchEffectKind::InitialDispatch || claim.mode != EffectClaimMode::Execute {
        return false;
    }
    let Some(delivery) = launch.delivery.as_ref() else {
        return false;
    };
    if delivery.state != PromptDeliveryState::RecoveryRequired {
        return false;
    }
    let recovery_confirmations = launch
        .confirmations
        .iter()
        .filter(|confirmation| confirmation.action == ConfirmationAction::RecoverPrompt)
        .count() as u64;
    recovery_confirmations <= u64::from(delivery.recovery_generation)
}

#[cfg(test)]
#[path = "worker_tests.rs"]
pub(crate) mod tests;
