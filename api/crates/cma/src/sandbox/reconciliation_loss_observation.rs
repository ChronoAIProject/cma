//! One identity-bearing outcome even when the binding deadline drops recovery.
use super::*;

pub(super) struct LossObservation<'a> {
    sandbox: &'a SandboxId,
    started: std::time::Instant,
    outcome: &'static str,
}
impl<'a> LossObservation<'a> {
    pub(super) fn new(sandbox: &'a SandboxId) -> Self {
        Self {
            sandbox,
            started: std::time::Instant::now(),
            outcome: "cancelled",
        }
    }
    pub(super) fn finish(&mut self, result: &Result<SandboxRuntimeLossOutcome, SandboxStoreError>) {
        self.outcome = match result {
            Ok(SandboxRuntimeLossOutcome::RecoveryScheduled) => "recovery_scheduled",
            Ok(SandboxRuntimeLossOutcome::Failed) => "failed",
            Ok(SandboxRuntimeLossOutcome::Superseded) => "superseded",
            Ok(SandboxRuntimeLossOutcome::NotEligible) => "not_eligible",
            Err(_) => "error",
        };
    }
}
impl Drop for LossObservation<'_> {
    fn drop(&mut self) {
        tracing::info!(sandbox_id = %self.sandbox, outcome = self.outcome,
            elapsed_ms = self.started.elapsed().as_millis() as u64,
            "runtime loss reconciliation outcome");
    }
}

impl SandboxReconciler {
    pub(crate) fn with_runtime_loss_bounded_proof(mut self, enabled: bool) -> Self {
        self.runtime_loss_bounded_proof_enabled = enabled;
        self
    }
    pub(super) async fn recover_missing(
        &self,
        sandbox: &Sandbox,
        observed_at: SystemTime,
    ) -> Result<bool, SandboxStoreError> {
        let mut observation = loss_observation::LossObservation::new(&sandbox.id);
        // The caller captured this boundary before its final absence proof.
        // Never advance it after provider I/O: doing so would let an older
        // Pending/absent observation authorize recovery over newer Agent work.
        let interruption_cause =
            interruption::observe(self.adapter.as_ref(), sandbox, observed_at).await;
        let outcome = measure(
            Stage::Transaction,
            self.store.recover_runtime_loss(SandboxRuntimeLossRequest {
                sandbox_id: sandbox.id.clone(),
                expected_revision: sandbox.revision,
                expected_runtime: sandbox.runtime.clone(),
                native_retirement_finality: false,
                retain_runtime: sandbox.lifecycle.home.is_some()
                    && sandbox
                        .runtime
                        .as_ref()
                        .is_some_and(|binding| binding.runtime_id.starts_with("agent-sandbox:")),
                interruption_cause,
                observed_at: rfc3339_at(observed_at),
            }),
            diagnostics::loss_outcome,
        )
        .await;
        observation.finish(&outcome);
        let outcome = outcome?;
        Ok(matches!(
            outcome,
            SandboxRuntimeLossOutcome::RecoveryScheduled | SandboxRuntimeLossOutcome::Failed
        ))
    }
}
