//! Fenced renewal and revocation of short-lived Trigger runtime policy.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use layer2_core::{
    rfc3339_at, ActivityAction, ActivityOutcome, ActivityService, CapabilityAuthorityAdmission,
    CapabilityRenewalRecord, CapabilityRevocationState, DetailKey, FailureClass, Launch,
    LaunchFailure, LaunchId, ReachStatus, RuntimePolicyObservation, TriggerService,
    TriggerServiceError, CAPABILITY_MAX_LIFETIME_SECONDS,
};

use super::github_effects::{CapabilityRenewal, CapabilityRevocation, GithubEffectExecutor};
use super::observability::{record_activity, TriggerAlert, OBSERVABILITY_SCHEMA_VERSION};
use super::worker::worker_record;

const DEFAULT_WORKER_LEASE: Duration = Duration::from_secs(120);
const RENEW_MARGIN: Duration = Duration::from_secs(20);

trait CapabilityClock: Send + Sync {
    fn now(&self) -> SystemTime;
}

struct SystemCapabilityClock;

impl CapabilityClock for SystemCapabilityClock {
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }
}

#[async_trait]
pub(crate) trait CapabilityExecutor: Send + Sync {
    async fn runtime_observation(
        &self,
        launch: &Launch,
        fence: u64,
    ) -> Result<RuntimePolicyObservation, LaunchFailure>;

    async fn admit_renewal(
        &self,
        launch: &Launch,
        fence: u64,
        target_policy_generation: u64,
        target_policy_expires_at_epoch_seconds: u64,
    ) -> Result<CapabilityAuthorityAdmission, LaunchFailure>;

    async fn renew(&self, launch: &Launch, fence: u64) -> Result<CapabilityRenewal, LaunchFailure>;

    async fn revoke(
        &self,
        launch: &Launch,
        fence: u64,
    ) -> Result<CapabilityRevocation, LaunchFailure>;

    async fn retire_credential(
        &self,
        launch: &Launch,
        fence: u64,
    ) -> Result<layer2_core::SessionId, LaunchFailure>;
}

#[async_trait]
impl CapabilityExecutor for GithubEffectExecutor {
    async fn runtime_observation(
        &self,
        launch: &Launch,
        fence: u64,
    ) -> Result<RuntimePolicyObservation, LaunchFailure> {
        self.capability_runtime_observation(launch, fence).await
    }

    async fn admit_renewal(
        &self,
        launch: &Launch,
        fence: u64,
        target_policy_generation: u64,
        target_policy_expires_at_epoch_seconds: u64,
    ) -> Result<CapabilityAuthorityAdmission, LaunchFailure> {
        self.admit_capability_renewal(
            launch,
            fence,
            target_policy_generation,
            target_policy_expires_at_epoch_seconds,
        )
        .await
    }

    async fn renew(&self, launch: &Launch, fence: u64) -> Result<CapabilityRenewal, LaunchFailure> {
        self.renew_capability(launch, fence).await
    }

    async fn revoke(
        &self,
        launch: &Launch,
        fence: u64,
    ) -> Result<CapabilityRevocation, LaunchFailure> {
        self.revoke_capability(launch, fence).await
    }

    async fn retire_credential(
        &self,
        launch: &Launch,
        fence: u64,
    ) -> Result<layer2_core::SessionId, LaunchFailure> {
        self.retire_session_credential(launch, fence).await
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CapabilityRunOutcome {
    NotAcquired,
    NoWork,
    Renewed,
    Revoked,
    Deferred,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RuntimeObservationDisposition {
    Current,
    Renew,
    PendingPriorInstall,
    AuthorityConflict,
    GenerationConflict,
}

enum RenewalPreparation {
    Complete(CapabilityRunOutcome),
    Admitted(Box<AdmittedRenewal>),
}

struct AdmittedRenewal {
    launch: Box<Launch>,
    lease: layer2_core::WorkerLease,
    reach: ReachStatus,
}

fn runtime_observation_disposition(
    launch: &Launch,
    reach: &ReachStatus,
    observed: &RuntimePolicyObservation,
) -> RuntimeObservationDisposition {
    if !observed.required || !observed.supported {
        return RuntimeObservationDisposition::AuthorityConflict;
    }
    if !observed.installed {
        return RuntimeObservationDisposition::Renew;
    }
    if observed.revoked || observed.launch_id.as_deref() != Some(launch.id.0.as_str()) {
        return RuntimeObservationDisposition::AuthorityConflict;
    }
    let exact_current = observed.runtime_audience == reach.runtime_audience
        && observed.generation == Some(reach.policy_generation)
        && observed.fence == Some(reach.policy_fence)
        && observed.expires_at_epoch_seconds == Some(reach.expires_at_epoch_seconds);
    if exact_current {
        return RuntimeObservationDisposition::Current;
    }
    let exact_pending = launch
        .capability_authority_admission
        .as_ref()
        .is_some_and(|admission| {
            observed.generation == Some(admission.target_policy_generation)
                && observed.fence == Some(admission.snapshot.fence)
                && observed.expires_at_epoch_seconds
                    == Some(admission.target_policy_expires_at_epoch_seconds)
        });
    if exact_pending {
        RuntimeObservationDisposition::PendingPriorInstall
    } else {
        RuntimeObservationDisposition::GenerationConflict
    }
}

/// One maintenance attempt. The shared Launch lease is persisted before any
/// provider or runtime call and supplies the policy fence for this attempt.
pub(crate) struct CapabilityWorker {
    triggers: Arc<TriggerService>,
    executor: Arc<dyn CapabilityExecutor>,
    holder: String,
    clock: Arc<dyn CapabilityClock>,
    lease_duration: Duration,
    renew_margin: Duration,
    activity: Option<Arc<ActivityService>>,
}

impl CapabilityWorker {
    pub(crate) fn new(
        triggers: Arc<TriggerService>,
        executor: Arc<dyn CapabilityExecutor>,
        holder: impl Into<String>,
    ) -> Result<Self, TriggerServiceError> {
        let holder = holder.into();
        if holder.trim().is_empty() || holder.len() > 256 {
            return Err(layer2_core::TriggerValidationError::field(
                "capability_worker.holder",
                "is not usable",
            )
            .into());
        }
        Ok(Self {
            triggers,
            executor,
            holder,
            clock: Arc::new(SystemCapabilityClock),
            lease_duration: DEFAULT_WORKER_LEASE,
            renew_margin: RENEW_MARGIN,
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
        executor: Arc<dyn CapabilityExecutor>,
        holder: impl Into<String>,
        clock: Arc<dyn CapabilityClock>,
    ) -> Result<Self, TriggerServiceError> {
        let mut worker = Self::new(triggers, executor, holder)?;
        worker.clock = clock;
        Ok(worker)
    }

    pub(crate) async fn run_once(
        &self,
        id: &LaunchId,
        expected_revision: u64,
        correlation_id: &str,
    ) -> Result<CapabilityRunOutcome, TriggerServiceError> {
        // Keep the large Launch state out of callers' async frames. Tokio's
        // worker threads use bounded stacks just like the test harness.
        Box::pin(self.run_once_inner(id, expected_revision, correlation_id)).await
    }

    #[inline(never)]
    async fn run_once_inner(
        &self,
        id: &LaunchId,
        expected_revision: u64,
        correlation_id: &str,
    ) -> Result<CapabilityRunOutcome, TriggerServiceError> {
        let acquired = self.clock.now();
        let acquired_at = rfc3339_at(acquired);
        let expires_at = rfc3339_at(acquired + self.lease_duration);
        let Some(mut launch) = self
            .triggers
            .acquire_worker(
                id,
                expected_revision,
                &self.holder,
                &acquired_at,
                &expires_at,
            )
            .await?
        else {
            return Ok(CapabilityRunOutcome::NotAcquired);
        };
        let lease = launch
            .lease
            .clone()
            .expect("a capability worker owns a persisted Launch lease");
        let cancellation_settled = match self
            .triggers
            .reconcile_cancelled_session(&mut launch, &acquired_at, correlation_id)
            .await
        {
            Ok(settled) => settled,
            Err(error) => {
                self.triggers
                    .release_worker(&launch.id, launch.revision_number, &lease)
                    .await?;
                return Err(error);
            }
        };
        if cancellation_settled {
            launch = self
                .triggers
                .commit_worker(launch.revision_number, &lease, launch)
                .await?;
        }
        let reach = launch.reach.clone();

        if self.triggers.rollout_policy().capability_must_revoke()
            && !launch.capability_requires_revocation()
            && reach
                .as_ref()
                .is_some_and(|reach| reach.state != CapabilityRevocationState::Revoked)
        {
            launch.request_capability_revocation(&acquired_at, correlation_id)?;
            launch = self
                .triggers
                .commit_worker(launch.revision_number, &lease, launch)
                .await?;
        }

        if launch.capability_requires_revocation()
            && launch.reach.as_ref().is_some_and(|reach| {
                !matches!(
                    reach.state,
                    CapabilityRevocationState::RevocationPending
                        | CapabilityRevocationState::Revoked
                )
            })
        {
            // Backfill the durable intent for legacy terminal or provider-
            // closed records before contacting either revocation boundary.
            launch.request_capability_revocation(&acquired_at, correlation_id)?;
            launch = self
                .triggers
                .commit_worker(launch.revision_number, &lease, launch)
                .await?;
        }

        if launch.capability_requires_revocation() {
            return match self.executor.revoke(&launch, lease.fencing_token).await {
                Ok(revocation) => {
                    let settled_at = rfc3339_at(self.clock.now());
                    launch.record_capability_revocation(
                        &lease,
                        revocation.generation,
                        revocation.fence,
                        revocation.runtime_audience,
                        revocation.tombstone_acknowledged,
                        revocation.agent_key_revocation_confirmed,
                        &settled_at,
                        correlation_id,
                    )?;
                    if revocation.agent_key_revocation_confirmed {
                        launch.record_session_credential_retirement(
                            &lease,
                            layer2_core::SessionCredentialRetirement {
                                session_id: revocation.credential_session_id,
                                settled_at: settled_at.clone(),
                            },
                        )?;
                    }
                    let released = self.commit_and_release(launch, &lease).await?;
                    self.record_action(
                        &released,
                        ActivityAction::TriggerCapabilityRevoke,
                        ActivityOutcome::Succeeded,
                        correlation_id,
                        lease.fencing_token,
                    )
                    .await;
                    Ok(CapabilityRunOutcome::Revoked)
                }
                Err(_) => {
                    let released = self.release(&launch, &lease).await?;
                    self.record_action(
                        &released,
                        ActivityAction::TriggerCapabilityRevoke,
                        ActivityOutcome::Failed,
                        correlation_id,
                        lease.fencing_token,
                    )
                    .await;
                    Ok(CapabilityRunOutcome::Deferred)
                }
            };
        }

        if launch.session_credential_requires_retirement() {
            return match self
                .executor
                .retire_credential(&launch, lease.fencing_token)
                .await
            {
                Ok(session_id) => {
                    let settled_at = rfc3339_at(self.clock.now());
                    launch.record_session_credential_retirement(
                        &lease,
                        layer2_core::SessionCredentialRetirement {
                            session_id,
                            settled_at,
                        },
                    )?;
                    let released = self.commit_and_release(launch, &lease).await?;
                    self.record_action(
                        &released,
                        ActivityAction::TriggerCapabilityRevoke,
                        ActivityOutcome::Succeeded,
                        correlation_id,
                        lease.fencing_token,
                    )
                    .await;
                    Ok(CapabilityRunOutcome::Revoked)
                }
                Err(_) => {
                    let released = self.release(&launch, &lease).await?;
                    self.record_action(
                        &released,
                        ActivityAction::TriggerCapabilityRevoke,
                        ActivityOutcome::Failed,
                        correlation_id,
                        lease.fencing_token,
                    )
                    .await;
                    Ok(CapabilityRunOutcome::Deferred)
                }
            };
        }

        let preparation = Box::pin(self.run_renewal_tail(
            launch,
            lease,
            reach,
            acquired,
            acquired_at,
            correlation_id,
        ))
        .await?;
        match preparation {
            RenewalPreparation::Complete(outcome) => Ok(outcome),
            RenewalPreparation::Admitted(admitted) => {
                Box::pin(self.settle_admitted_renewal(*admitted, correlation_id)).await
            }
        }
    }

    #[inline(never)]
    async fn run_renewal_tail(
        &self,
        mut launch: Launch,
        lease: layer2_core::WorkerLease,
        reach: Option<ReachStatus>,
        acquired: SystemTime,
        acquired_at: String,
        correlation_id: &str,
    ) -> Result<RenewalPreparation, TriggerServiceError> {
        let Some(reach) = reach else {
            self.release(&launch, &lease).await?;
            return Ok(RenewalPreparation::Complete(CapabilityRunOutcome::NoWork));
        };

        if reach.state == CapabilityRevocationState::Revoked {
            self.release(&launch, &lease).await?;
            return Ok(RenewalPreparation::Complete(CapabilityRunOutcome::NoWork));
        }

        let now_epoch_seconds = acquired
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let due_by_time = reach.state == CapabilityRevocationState::Expired
            || reach.expires_at_epoch_seconds
                <= now_epoch_seconds.saturating_add(self.renew_margin.as_secs());
        if !due_by_time {
            let observed = match self
                .executor
                .runtime_observation(&launch, lease.fencing_token)
                .await
            {
                Ok(observed) => observed,
                Err(_) => {
                    let released = self.release(&launch, &lease).await?;
                    self.record_action(
                        &released,
                        ActivityAction::TriggerCapabilityRenew,
                        ActivityOutcome::Failed,
                        correlation_id,
                        lease.fencing_token,
                    )
                    .await;
                    return Ok(RenewalPreparation::Complete(CapabilityRunOutcome::Deferred));
                }
            };
            let disposition = runtime_observation_disposition(&launch, &reach, &observed);
            match disposition {
                RuntimeObservationDisposition::Current => {
                    let changed = launch.record_capability_refusal_observation(
                        &lease,
                        observed.last_refusal,
                        &acquired_at,
                        correlation_id,
                    )?;
                    if changed {
                        self.commit_and_release(launch, &lease).await?;
                    } else {
                        self.release(&launch, &lease).await?;
                    }
                    return Ok(RenewalPreparation::Complete(CapabilityRunOutcome::NoWork));
                }
                RuntimeObservationDisposition::Renew => {}
                RuntimeObservationDisposition::PendingPriorInstall => {
                    let released = self.release(&launch, &lease).await?;
                    self.record_action(
                        &released,
                        ActivityAction::TriggerCapabilityRenew,
                        ActivityOutcome::Failed,
                        correlation_id,
                        lease.fencing_token,
                    )
                    .await;
                    return Ok(RenewalPreparation::Complete(CapabilityRunOutcome::Deferred));
                }
                RuntimeObservationDisposition::AuthorityConflict
                | RuntimeObservationDisposition::GenerationConflict => {
                    let authority_conflict = matches!(
                        disposition,
                        RuntimeObservationDisposition::AuthorityConflict
                    );
                    launch.fail(
                        LaunchFailure {
                            class: if authority_conflict {
                                FailureClass::TerminalSecurity
                            } else {
                                FailureClass::RecoveryRequired
                            },
                            code: if authority_conflict {
                                "trigger_runtime_policy_authority_conflict"
                            } else {
                                "trigger_runtime_policy_generation_conflict"
                            }
                            .into(),
                            detail: "The runtime policy observation contradicted the retained Launch authority."
                                .into(),
                            remediation: "Inspect the retained runtime and policy evidence; CMA will not overwrite the observed policy automatically."
                                .into(),
                            correlation_id: correlation_id.into(),
                            retry_at: None,
                            observed_at: acquired_at.clone(),
                        },
                        &acquired_at,
                    )?;
                    let released = self.commit_and_release(launch, &lease).await?;
                    self.record_action(
                        &released,
                        ActivityAction::TriggerCapabilityRenew,
                        ActivityOutcome::Failed,
                        correlation_id,
                        lease.fencing_token,
                    )
                    .await;
                    return Ok(RenewalPreparation::Complete(if authority_conflict {
                        CapabilityRunOutcome::Failed
                    } else {
                        CapabilityRunOutcome::Deferred
                    }));
                }
            }
        }

        // A persisted admission means a prior policy install may have crossed
        // the runtime boundary. Takeover never resends that generation. It
        // waits until the admitted TTL is past, then reserves a strictly later
        // generation under its own fresh fence.
        let pending_generation = launch
            .capability_authority_admission
            .as_ref()
            .map_or(0, |admission| admission.target_policy_generation);
        if launch
            .capability_authority_admission
            .as_ref()
            .is_some_and(|admission| {
                admission.target_policy_expires_at_epoch_seconds > now_epoch_seconds
            })
        {
            let released = self.release(&launch, &lease).await?;
            self.record_action(
                &released,
                ActivityAction::TriggerCapabilityRenew,
                ActivityOutcome::Failed,
                correlation_id,
                lease.fencing_token,
            )
            .await;
            return Ok(RenewalPreparation::Complete(CapabilityRunOutcome::Deferred));
        }
        let target_policy_generation = reach
            .policy_generation
            .max(pending_generation)
            .checked_add(1)
            .ok_or_else(|| {
                layer2_core::TriggerValidationError::field(
                    "capability.policy_generation",
                    "generation counter is exhausted",
                )
            })?;
        let target_policy_expires_at_epoch_seconds = now_epoch_seconds
            .checked_add(CAPABILITY_MAX_LIFETIME_SECONDS)
            .ok_or_else(|| {
                layer2_core::TriggerValidationError::field(
                    "capability.expires_at",
                    "policy lifetime exceeds the supported clock",
                )
            })?;
        let admission = match self
            .executor
            .admit_renewal(
                &launch,
                lease.fencing_token,
                target_policy_generation,
                target_policy_expires_at_epoch_seconds,
            )
            .await
        {
            Ok(admission) => admission,
            Err(_) => {
                let released = self.release(&launch, &lease).await?;
                self.record_action(
                    &released,
                    ActivityAction::TriggerCapabilityRenew,
                    ActivityOutcome::Failed,
                    correlation_id,
                    lease.fencing_token,
                )
                .await;
                return Ok(RenewalPreparation::Complete(CapabilityRunOutcome::Deferred));
            }
        };
        launch.record_capability_authority_admission(&lease, admission, acquired_at.clone())?;
        // A failed CAS is positive proof that no policy install was admitted.
        launch = self
            .triggers
            .commit_worker(launch.revision_number, &lease, launch)
            .await?;

        Ok(RenewalPreparation::Admitted(Box::new(AdmittedRenewal {
            launch: Box::new(launch),
            lease,
            reach,
        })))
    }

    #[inline(never)]
    async fn settle_admitted_renewal(
        &self,
        admitted: AdmittedRenewal,
        correlation_id: &str,
    ) -> Result<CapabilityRunOutcome, TriggerServiceError> {
        let AdmittedRenewal {
            launch,
            lease,
            reach,
        } = admitted;
        let mut launch = *launch;
        match self.executor.renew(&launch, lease.fencing_token).await {
            Ok(renewal) => {
                let settled_at = rfc3339_at(self.clock.now());
                launch.record_capability_renewal(
                    &lease,
                    CapabilityRenewalRecord {
                        reach: renewal.reach,
                        connection: renewal.connection,
                        authority: renewal.authority,
                        branch: renewal.branch,
                    },
                    &settled_at,
                    correlation_id,
                )?;
                let released = self.commit_and_release(launch, &lease).await?;
                self.record_action(
                    &released,
                    ActivityAction::TriggerCapabilityRenew,
                    ActivityOutcome::Succeeded,
                    correlation_id,
                    lease.fencing_token,
                )
                .await;
                Ok(CapabilityRunOutcome::Renewed)
            }
            Err(failure) => {
                let settled = self.clock.now();
                let settled_at = rfc3339_at(settled);
                let settled_epoch = settled
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                let mut changed = false;
                if reach.state == CapabilityRevocationState::Active
                    && reach.expires_at_epoch_seconds <= settled_epoch
                {
                    launch.record_capability_expired(
                        &lease,
                        settled_epoch,
                        &settled_at,
                        correlation_id,
                    )?;
                    changed = true;
                }
                let terminal_or_recoverable = !matches!(
                    failure.class,
                    FailureClass::RetryableTransient
                        | FailureClass::AmbiguousExternal
                        | FailureClass::ResourceQuota
                );
                if terminal_or_recoverable
                    && launch
                        .failure
                        .as_ref()
                        .is_none_or(|current| current.code != failure.code)
                {
                    let failed = matches!(
                        failure.class,
                        FailureClass::TerminalSecurity | FailureClass::TerminalValidation
                    );
                    launch.fail(failure, &settled_at)?;
                    changed = true;
                    let released = if changed {
                        self.commit_and_release(launch, &lease).await?
                    } else {
                        self.release(&launch, &lease).await?
                    };
                    self.record_action(
                        &released,
                        ActivityAction::TriggerCapabilityRenew,
                        ActivityOutcome::Failed,
                        correlation_id,
                        lease.fencing_token,
                    )
                    .await;
                    return Ok(if failed {
                        CapabilityRunOutcome::Failed
                    } else {
                        CapabilityRunOutcome::Deferred
                    });
                }
                let released = if changed {
                    self.commit_and_release(launch, &lease).await?
                } else {
                    self.release(&launch, &lease).await?
                };
                self.record_action(
                    &released,
                    ActivityAction::TriggerCapabilityRenew,
                    ActivityOutcome::Failed,
                    correlation_id,
                    lease.fencing_token,
                )
                .await;
                Ok(CapabilityRunOutcome::Deferred)
            }
        }
    }

    async fn commit_and_release(
        &self,
        launch: Launch,
        lease: &layer2_core::WorkerLease,
    ) -> Result<Launch, TriggerServiceError> {
        // Both store futures retain a full Launch across an await. Keep them
        // off Tokio's bounded worker stack just as run_once does for this worker.
        let committed = Box::pin(self.triggers.commit_worker(
            launch.revision_number,
            lease,
            launch,
        ))
        .await?;
        let released = Box::pin(self.triggers.release_worker(
            &committed.id,
            committed.revision_number,
            lease,
        ))
        .await?;
        Ok(released)
    }

    async fn release(
        &self,
        launch: &Launch,
        lease: &layer2_core::WorkerLease,
    ) -> Result<Launch, TriggerServiceError> {
        self.triggers
            .release_worker(&launch.id, launch.revision_number, lease)
            .await
    }

    async fn record_action(
        &self,
        launch: &Launch,
        action: ActivityAction,
        outcome: ActivityOutcome,
        correlation_id: &str,
        fence: u64,
    ) {
        let generation = launch
            .reach
            .as_ref()
            .map_or(0, |reach| reach.policy_generation);
        tracing::info!(
            event_name = "cma_trigger_capability_settlement",
            schema_version = OBSERVABILITY_SCHEMA_VERSION,
            launch_id = %launch.id.0,
            correlation_id,
            action = ?action,
            outcome = ?outcome,
            policy_generation = generation,
            fence,
            "Trigger capability worker settled one durable boundary"
        );
        if outcome == ActivityOutcome::Failed
            && launch
                .failure
                .as_ref()
                .is_some_and(|failure| failure.class == FailureClass::TerminalSecurity)
        {
            let alert = TriggerAlert::UnauthorizedEffect;
            let failure_code = launch
                .failure
                .as_ref()
                .map_or("none", |failure| failure.code.as_str());
            tracing::error!(
                event_name = "cma_trigger_alert_candidate",
                schema_version = OBSERVABILITY_SCHEMA_VERSION,
                alert_id = alert.id(),
                severity = alert.severity(),
                launch_id = %launch.id.0,
                correlation_id,
                action = ?action,
                fence,
                policy_generation = generation,
                failure_code,
                "Trigger unauthorized capability-effect alert candidate observed"
            );
        }
        if let Some(activity) = self.activity.as_ref() {
            record_activity(
                activity,
                worker_record(launch, action, outcome, correlation_id)
                    .with(DetailKey::PolicyGeneration, generation.to_string())
                    .with(DetailKey::FencingToken, fence.to_string()),
            )
            .await;
        }
    }
}

#[cfg(test)]
#[path = "capability_worker_tests.rs"]
mod tests;
