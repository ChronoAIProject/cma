//! The ordinary and lifecycle publishers use the same durable retry policy.
use super::{support::WorkerFailure, SandboxWorker};
use crate::sandbox::work_retry::{sandbox_class, RetryTelemetry};
use layer2_core::sandbox::work_retry::{work_retry_policy, WorkRetryDecision};
use layer2_core::sandbox::Sandbox;
use std::time::{Duration, SystemTime};

impl SandboxWorker {
    pub(super) fn retry_decision(
        &self,
        sandbox: &Sandbox,
        failure: &WorkerFailure,
        base: Duration,
    ) -> WorkRetryDecision {
        if !self.config.work_retry_policy_enabled {
            return WorkRetryDecision {
                delay: base,
                parked: false,
            };
        }
        work_retry_policy(
            sandbox_class(sandbox),
            failure.retry_kind(),
            sandbox.work.attempts,
            base,
        )
    }

    pub(super) fn retry_telemetry(
        &self,
        sandbox: &Sandbox,
        failure: &WorkerFailure,
        decision: WorkRetryDecision,
        now: SystemTime,
    ) -> Option<RetryTelemetry> {
        self.config.work_retry_policy_enabled.then(|| {
            RetryTelemetry::sandbox(
                sandbox,
                failure.retry_kind(),
                decision,
                now + decision.delay,
            )
        })
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
