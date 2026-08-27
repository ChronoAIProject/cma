//! Fenced renewal and revocation of short-lived Trigger runtime policy.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use layer2_core::{
    rfc3339_at, CapabilityRenewalRecord, CapabilityRevocationState, FailureClass, Launch,
    LaunchFailure, LaunchId, TriggerService, TriggerServiceError,
};

use super::github_effects::{CapabilityRenewal, CapabilityRevocation, GithubEffectExecutor};

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
    async fn runtime_audience(&self, launch: &Launch, fence: u64) -> Result<String, LaunchFailure>;

    async fn renew(&self, launch: &Launch, fence: u64) -> Result<CapabilityRenewal, LaunchFailure>;

    async fn revoke(
        &self,
        launch: &Launch,
        fence: u64,
    ) -> Result<CapabilityRevocation, LaunchFailure>;
}

#[async_trait]
impl CapabilityExecutor for GithubEffectExecutor {
    async fn runtime_audience(&self, launch: &Launch, fence: u64) -> Result<String, LaunchFailure> {
        self.capability_runtime_audience(launch, fence).await
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

/// One maintenance attempt. The shared Launch lease is persisted before any
/// provider or runtime call and supplies the policy fence for this attempt.
pub(crate) struct CapabilityWorker {
    triggers: Arc<TriggerService>,
    executor: Arc<dyn CapabilityExecutor>,
    holder: String,
    clock: Arc<dyn CapabilityClock>,
    lease_duration: Duration,
    renew_margin: Duration,
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
        })
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
        let Some(reach) = launch.reach.clone() else {
            self.release(&launch, &lease).await?;
            return Ok(CapabilityRunOutcome::NoWork);
        };

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
                    self.commit_and_release(launch, &lease).await?;
                    Ok(CapabilityRunOutcome::Revoked)
                }
                Err(_) => {
                    self.release(&launch, &lease).await?;
                    Ok(CapabilityRunOutcome::Deferred)
                }
            };
        }

        if reach.state == CapabilityRevocationState::Revoked {
            self.release(&launch, &lease).await?;
            return Ok(CapabilityRunOutcome::NoWork);
        }

        let now_epoch_seconds = acquired
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let due_by_time = reach.state == CapabilityRevocationState::Expired
            || reach.expires_at_epoch_seconds
                <= now_epoch_seconds.saturating_add(self.renew_margin.as_secs());
        let runtime_changed = if due_by_time {
            false
        } else {
            match self
                .executor
                .runtime_audience(&launch, lease.fencing_token)
                .await
            {
                Ok(current) => current != reach.runtime_audience,
                Err(_) => {
                    self.release(&launch, &lease).await?;
                    return Ok(CapabilityRunOutcome::Deferred);
                }
            }
        };
        if !due_by_time && !runtime_changed {
            self.release(&launch, &lease).await?;
            return Ok(CapabilityRunOutcome::NoWork);
        }

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
                self.commit_and_release(launch, &lease).await?;
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
                    if changed {
                        self.commit_and_release(launch, &lease).await?;
                    }
                    return Ok(if failed {
                        CapabilityRunOutcome::Failed
                    } else {
                        CapabilityRunOutcome::Deferred
                    });
                }
                if changed {
                    self.commit_and_release(launch, &lease).await?;
                } else {
                    self.release(&launch, &lease).await?;
                }
                Ok(CapabilityRunOutcome::Deferred)
            }
        }
    }

    async fn commit_and_release(
        &self,
        launch: Launch,
        lease: &layer2_core::WorkerLease,
    ) -> Result<(), TriggerServiceError> {
        let committed = self
            .triggers
            .commit_worker(launch.revision_number, lease, launch)
            .await?;
        self.triggers
            .release_worker(&committed.id, committed.revision_number, lease)
            .await?;
        Ok(())
    }

    async fn release(
        &self,
        launch: &Launch,
        lease: &layer2_core::WorkerLease,
    ) -> Result<(), TriggerServiceError> {
        self.triggers
            .release_worker(&launch.id, launch.revision_number, lease)
            .await?;
        Ok(())
    }
}

#[cfg(test)]
#[path = "capability_worker_tests.rs"]
mod tests;
