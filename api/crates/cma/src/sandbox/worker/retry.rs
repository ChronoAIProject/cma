//! The ordinary and lifecycle publishers use the same durable retry policy.
use super::{support::WorkerFailure, SandboxWorker};
use crate::sandbox::work_retry::{sandbox_class, RetryTelemetry};
use layer2_core::sandbox::work_retry::{work_retry_policy, WorkRetryDecision};
use layer2_core::sandbox::{Sandbox, SandboxLifecycleOperation, SandboxStatus};
use layer3_adapter::{RuntimeHandle, RuntimeProvisionRequest, RuntimeStatus};
use std::time::{Duration, SystemTime};

impl SandboxWorker {
    pub(super) async fn initial_provisioned_runtime_status(
        &self,
        sandbox: &Sandbox,
        request: &RuntimeProvisionRequest,
        handle: &RuntimeHandle,
    ) -> layer3_adapter::Result<Option<RuntimeStatus>> {
        let status = super::super::runtime_attachment::provisioned_runtime_status(
            self.dependencies.adapter.as_ref(),
            &request.spec,
            handle,
            request.generation,
        );
        if !self.config.native_create_retry_enabled
            || request.generation != 1
            || sandbox.runtime.is_some()
            || sandbox.lifecycle.operation.is_some()
        {
            return status.await;
        }
        let Some(home) = request.spec.home_volume.as_deref().filter(|home| {
            home.request.policy.is_native_vct() && home.receipt.native_authority.is_some()
        }) else {
            return status.await;
        };
        layer3_adapter::agent_sandbox::with_initial_relay_readiness_retry(
            home.clone(),
            relay_work_deadline(sandbox),
            status,
        )
        .await
    }

    pub(super) fn is_image_rotation(sandbox: &Sandbox) -> bool {
        sandbox.status == SandboxStatus::Rotating
            && matches!(
                sandbox.lifecycle.operation,
                Some(SandboxLifecycleOperation::ImageRotation { .. })
            )
    }

    pub(super) fn retry_decision(
        &self,
        sandbox: &Sandbox,
        failure: &WorkerFailure,
        base: Duration,
    ) -> WorkRetryDecision {
        if self.native_retry_class(sandbox, failure).is_some() {
            return WorkRetryDecision {
                delay: base.min(Duration::from_secs(1)),
                parked: false,
            };
        }
        let floor = if failure.is_startup_hold() {
            super::startup_hold::delay(base, sandbox.work.attempts)
        } else {
            base
        };
        if !self.config.work_retry_policy_enabled && !Self::is_image_rotation(sandbox) {
            return WorkRetryDecision {
                delay: floor,
                parked: false,
            };
        }
        let mut decision = work_retry_policy(
            sandbox_class(sandbox),
            failure.retry_kind(),
            sandbox.work.attempts,
            base,
        );
        if failure.is_startup_hold() {
            decision.delay = decision.delay.max(floor);
        }
        decision
    }

    pub(super) fn retry_telemetry(
        &self,
        sandbox: &Sandbox,
        failure: &WorkerFailure,
        decision: WorkRetryDecision,
        now: SystemTime,
    ) -> Option<RetryTelemetry> {
        (self.config.work_retry_policy_enabled || Self::is_image_rotation(sandbox)).then(|| {
            RetryTelemetry::sandbox(
                sandbox,
                failure.retry_kind(),
                decision,
                now + decision.delay,
            )
        })
    }
}

impl SandboxWorker {
    pub(super) fn native_retry_class(
        &self,
        sandbox: &Sandbox,
        failure: &WorkerFailure,
    ) -> Option<&'static str> {
        use layer2_core::sandbox::{SandboxStatus, SandboxWorkStep};
        let home = sandbox.lifecycle.home.as_deref()?;
        let native = home.native_pool.as_deref()?;
        // Only expected initial readiness can spend this durable retry budget.
        // A transport outage or observed crash must retain the ordinary backoff.
        (self.config.native_create_retry_enabled
            && failure.native_claim_pending
            && sandbox.status == SandboxStatus::Provisioning
            && sandbox.work.step == SandboxWorkStep::ProvisionRuntime
            && sandbox.work.attempts < 5
            && sandbox.lifecycle.operation.is_none()
            && sandbox.lifecycle.suspended_runtime.is_none()
            && sandbox.lifecycle.retirement_archive.is_none()
            && sandbox.lifecycle.interruption.is_none()
            && sandbox.lifecycle.home_recovery.is_none()
            && sandbox.lifecycle.unpublished_recovery.is_none()
            && sandbox.lifecycle.runtime_update.is_none()
            && sandbox.lifecycle.runtime_generation_high_water <= 1
            && sandbox.credential.key_generation == 0
            && !sandbox.credential.delivered
            && sandbox.credential.pending.is_none()
            && sandbox.credential.current.is_none()
            && native.claim.is_some()
            && !native.cleaned
            && native.cleanup.is_none()
            && !home.released
            && home.request.policy.is_native_vct()
            && home.validate(&sandbox.id).is_ok())
        .then_some(
            if sandbox
                .profile
                .content
                .environment_variables
                .iter()
                .any(|entry| entry.name == "CMA_MACHINE_INCLUDED" && entry.value == "1")
            {
                "machine"
            } else {
                "plain"
            },
        )
    }

    pub(super) async fn handle_provision_failure(
        &self,
        sandbox: Sandbox,
        failure: WorkerFailure,
        now: SystemTime,
        started: tokio::time::Instant,
    ) -> Result<(), layer2_core::sandbox::SandboxStoreError> {
        // Readiness has already consumed seconds. A shortened delay must start
        // at observation completion, never at the stale worker claim timestamp.
        let observed_at = if self.native_retry_class(&sandbox, &failure).is_some() {
            rounded_observation(now + started.elapsed())
        } else {
            now
        };
        self.handle_failure(sandbox, failure, observed_at).await
    }

    #[cfg(test)]
    pub(in crate::sandbox) fn native_retry_delay_for_test(
        &self,
        sandbox: &Sandbox,
        error: layer3_adapter::AdapterError,
    ) -> Duration {
        self.retry_decision(
            sandbox,
            &super::support::adapter_failure(error),
            self.config.retry_delay,
        )
        .delay
    }
}

fn rounded_observation(observed: SystemTime) -> SystemTime {
    // Durable due times have whole-second precision. Round up so truncation
    // cannot turn a one-second delay into an immediate scheduler retry.
    let elapsed = observed
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    SystemTime::UNIX_EPOCH
        + Duration::from_secs(elapsed.as_secs() + u64::from(elapsed.subsec_nanos() > 0))
}

fn relay_work_deadline(
    sandbox: &Sandbox,
) -> impl Fn() -> layer3_adapter::Result<Option<tokio::time::Instant>> + 'static {
    use layer2_core::sandbox::store::WorkAuthority;
    let legacy = sandbox.work.lease.as_ref().map(|lease| {
        let expires_at = layer2_core::seconds_from_rfc3339(&lease.expires_at)?;
        let expires_at = SystemTime::UNIX_EPOCH.checked_add(Duration::from_secs(expires_at))?;
        let remaining = expires_at.duration_since(SystemTime::now()).ok()?;
        tokio::time::Instant::now().checked_add(remaining)
    });
    move || -> layer3_adapter::Result<_> {
        let expired =
            || layer3_adapter::AdapterError::Unavailable("Sandbox work authority expired".into());
        let mut earliest: Option<tokio::time::Instant> = None;
        for authority in WorkAuthority::current().map_err(|_| expired())? {
            let deadline = authority.deadline().map_err(|_| expired())?;
            earliest = Some(earliest.map_or(deadline, |at| at.min(deadline)));
        }
        match (earliest, legacy) {
            (Some(deadline), _) => Ok(Some(deadline)),
            (None, Some(Some(deadline))) if deadline > tokio::time::Instant::now() => {
                Ok(Some(deadline))
            }
            (None, Some(_)) => Err(expired()),
            (None, None) => Ok(None),
        }
    }
}

pub(super) fn native_retry_published(class: Option<&'static str>) {
    if let Some(class) = class {
        metrics::counter!("cma_native_create_readiness_retry_total", "class" => class).increment(1);
        tracing::info!(
            class,
            "Initial native readiness retry published; original Claim retained"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::super::support::{
        adapter_failure, credential_failure, dependency_failure, WorkerFailure,
    };
    use crate::sandbox::{credential::CredentialError, worker::SandboxDependencyError};
    use layer2_core::sandbox::work_retry::WorkRetryFailure;
    use layer3_adapter::AdapterError;

    #[test]
    fn native_create_retry_rounds_completion_up_to_durable_clock_precision() {
        use std::time::{Duration, SystemTime};
        let at = SystemTime::UNIX_EPOCH + Duration::from_secs(100);
        assert_eq!(super::rounded_observation(at), at);
        assert_eq!(
            super::rounded_observation(at + Duration::from_millis(1)),
            at + Duration::from_secs(1)
        );
        assert_eq!(
            super::rounded_observation(at + Duration::from_millis(999)),
            at + Duration::from_secs(1)
        );
    }

    #[test]
    fn work_retry_classifies_external_refusals_without_changing_terminal_disposition() {
        let security = credential_failure(CredentialError::Security("fixture refusal".into()));
        assert_eq!(security.retry_kind(), WorkRetryFailure::ExternalAction);
        assert!(!security.is_retryable());
        let replaced = adapter_failure(AdapterError::Unavailable(
            "retiring native runtime process was replaced".into(),
        ));
        assert!(replaced.is_retryable());
        assert_eq!(replaced.retry_kind(), WorkRetryFailure::ExternalAction);
        assert_eq!(
            dependency_failure(SandboxDependencyError::AgentsStopping).retry_kind(),
            WorkRetryFailure::Waiting
        );
        assert_eq!(
            dependency_failure(SandboxDependencyError::Transient(
                "sandbox agents are still stopping".into()
            ))
            .retry_kind(),
            WorkRetryFailure::Waiting
        );
        assert_eq!(
            WorkerFailure::waiting("writer resume pending").retry_kind(),
            WorkRetryFailure::Waiting
        );
        assert_eq!(
            adapter_failure(AdapterError::Unavailable("network".into())).retry_kind(),
            WorkRetryFailure::Transient
        );
    }
}
