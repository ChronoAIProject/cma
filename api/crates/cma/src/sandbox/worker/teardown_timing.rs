//! Per-attempt wall time and fixed outcomes; never completion or CPU attribution.
use crate::sandbox::funding::FundingError;
use layer2_core::SandboxId;
use layer3_adapter::AdapterError;
use sha2::{Digest, Sha256};
use std::{future::Future, time::Instant};

pub(super) enum Phase {
    RetainedHomeRoute,
    RetainedRuntime,
    AdoptRuntime,
    RuntimeInventory,
    Runtime,
    Funding,
    Publication,
}
impl Phase {
    fn label(&self) -> &'static str {
        match self {
            Self::RetainedHomeRoute => "retained_home_route",
            Self::RetainedRuntime => "retained_runtime",
            Self::AdoptRuntime => "adopt_runtime",
            Self::RuntimeInventory => "runtime_inventory",
            Self::Runtime => "runtime",
            Self::Funding => "funding",
            Self::Publication => "publication",
        }
    }
}

pub(super) struct TeardownTiming {
    sandbox_sha256: String,
    attempt: u32,
}
impl TeardownTiming {
    pub(super) fn new(id: &SandboxId, attempt: u32) -> Self {
        Self {
            sandbox_sha256: format!("{:x}", Sha256::digest(id.as_str().as_bytes())),
            attempt,
        }
    }

    pub(super) async fn observe<T, E>(
        &self,
        phase: Phase,
        future: impl Future<Output = Result<T, E>>,
        classify: fn(&E) -> &'static str,
    ) -> Result<T, E> {
        let started = Instant::now();
        let result = future.await;
        let outcome = result.as_ref().err().map(classify).unwrap_or("ok");
        tracing::info!(sandbox_id_sha256 = %self.sandbox_sha256, attempt = self.attempt,
            phase = phase.label(), outcome, elapsed_ms = started.elapsed().as_millis() as u64,
            "Sandbox teardown phase observed");
        result
    }
}

pub(super) fn adapter_outcome(error: &AdapterError) -> &'static str {
    match error {
        AdapterError::AccessRevoked => "access_revoked",
        AdapterError::Unavailable(_) | AdapterError::RuntimeStartupHold(_) => "unavailable",
        AdapterError::NativeClaimNotReady(_) => "claim_pending",
        AdapterError::NativePoolDefinitionAbsent { .. } => "pool_definition_absent",
        AdapterError::RuntimeLedgerHold(_) => "ledger_hold",
        AdapterError::RuntimeCapacity(_) | AdapterError::AtCapacity(_) => "capacity",
        AdapterError::InputAuthorityUnavailable(_) | AdapterError::PromptNotWritten(_) => {
            "authority_unavailable"
        }
        AdapterError::Rejected(_)
        | AdapterError::AdmittedSandboxSpecDrift
        | AdapterError::NativePoolConfigurationDrift
        | AdapterError::DesktopFailed
        | AdapterError::SkillUpgradeRefused(_)
        | AdapterError::OperationIdentityConflict(_)
        | AdapterError::Unsupported(_) => "rejected",
        AdapterError::RuntimeContractMismatch(_) => "contract_mismatch",
    }
}

pub(super) fn funding_outcome(error: &FundingError) -> &'static str {
    match error {
        FundingError::NotEnrolled => "not_enrolled",
        FundingError::NotFound => "not_found",
        FundingError::Conflict => "conflict",
        FundingError::RateLimited(_) => "rate_limited",
        FundingError::Invalid(_) => "invalid",
        FundingError::Unfunded(_) => "unfunded",
        FundingError::Retryable(_) => "retryable",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn teardown_observation_preserves_success_and_exact_failure() {
        let timing = TeardownTiming::new(&SandboxId("fixture".into()), 0);
        let ok = timing
            .observe(
                Phase::Runtime,
                async { Ok::<_, AdapterError>(42) },
                adapter_outcome,
            )
            .await;
        assert_eq!(ok.unwrap(), 42);
        let error = timing
            .observe(
                Phase::Runtime,
                async { Err::<(), _>(AdapterError::RuntimeLedgerHold("private detail".into())) },
                adapter_outcome,
            )
            .await
            .unwrap_err();
        assert_eq!(adapter_outcome(&error), "ledger_hold");
        assert!(
            matches!(error, AdapterError::RuntimeLedgerHold(detail) if detail == "private detail")
        );
        assert_eq!(funding_outcome(&FundingError::Conflict), "conflict");
        assert_eq!(
            adapter_outcome(&AdapterError::Unavailable("private detail".into())),
            "unavailable"
        );
    }

    #[tokio::test]
    async fn teardown_observation_keeps_access_revocation_terminal() {
        let timing = TeardownTiming::new(&SandboxId("fixture".into()), 0);
        let error = timing
            .observe(
                Phase::Runtime,
                async { Err::<(), _>(AdapterError::AccessRevoked) },
                adapter_outcome,
            )
            .await
            .unwrap_err();
        assert_eq!(error, AdapterError::AccessRevoked);
        assert_eq!(adapter_outcome(&error), "access_revoked");
        let failure = super::super::support::adapter_failure(error);
        assert_eq!(failure.typed().class, "terminal_security");
        assert!(!failure.is_retryable());
    }
}
