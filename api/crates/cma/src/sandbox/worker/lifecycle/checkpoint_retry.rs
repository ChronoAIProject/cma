//! Retry checkpoint failure only after the exact writer hold is resumed.
use super::super::{support::WorkerFailure, SandboxWorker};
use super::support::model_error;
use layer2_core::{
    rfc3339_at,
    sandbox::{store::SandboxStoreError, Sandbox},
};
use std::time::SystemTime;

impl SandboxWorker {
    pub(super) async fn fail_after_writer_resume(
        &self,
        sandbox: Sandbox,
        failure: WorkerFailure,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        // Retry delay starts after writer recovery, not before its potentially slow acknowledgement.
        let started = tokio::time::Instant::now();
        let resumed = self.resume_lifecycle_writers(&sandbox).await;
        let now = now + started.elapsed();
        if resumed.is_err() {
            return self
                .handle_lifecycle_failure(
                    sandbox,
                    WorkerFailure::waiting("checkpoint writer resumption is unconfirmed"),
                    now,
                )
                .await;
        }
        if failure.is_retryable() {
            let decision = self.retry_decision(&sandbox, &failure, self.config.retry_delay);
            let telemetry = self.retry_telemetry(&sandbox, &failure, decision, now);
            let stamp = rfc3339_at(now);
            let lease = super::support::lifecycle_lease(&sandbox, &self.config.holder)?;
            let mut next = sandbox.clone();
            if self.should_park_rotation(&sandbox, decision) {
                next.park_runtime_update_after_writer_resume(
                    failure.typed(),
                    &lease.holder,
                    lease.fence,
                    stamp.clone(),
                )
            } else {
                next.retry_lifecycle_after_writer_resume(
                    rfc3339_at(now + decision.delay),
                    &lease.holder,
                    lease.fence,
                    stamp.clone(),
                )
            }
            .map_err(model_error)?;
            self.commit_lifecycle_phase(
                &sandbox,
                next,
                Vec::new(),
                layer2_core::sandbox::store::SandboxLifecycleAgentScope::Touched,
                stamp,
                vec![layer2_core::sandbox::SandboxEventKind::WorkStepFailed {
                    step: layer2_core::sandbox::SandboxWorkStep::RunLifecycleOperation,
                    failure: failure.typed(),
                }],
            )
            .await?;
            if let Some(telemetry) = telemetry {
                telemetry.published();
            }
            return Ok(());
        }
        self.handle_lifecycle_failure(sandbox, failure, now).await
    }
}
