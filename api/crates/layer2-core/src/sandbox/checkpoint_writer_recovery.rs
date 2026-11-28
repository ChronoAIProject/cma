use super::{
    Sandbox, SandboxImageRotationPhase, SandboxLifecycleOperation, SandboxModelError,
    SandboxPausePhase, TypedFailure,
};

impl Sandbox {
    /// A resumed token is permanently retired by the runtime; retries need new authority.
    pub fn retry_lifecycle_after_writer_resume(
        &mut self,
        ready_at: String,
        holder: &str,
        fence: u64,
        at: String,
    ) -> Result<(), SandboxModelError> {
        self.require_lifecycle_authority(holder, fence)?;
        let mut next = self.clone();
        next.renew_checkpoint_writer_authority()?;
        next.retry_lifecycle(ready_at, holder, fence, at)?;
        *self = next;
        Ok(())
    }

    pub fn park_runtime_update_after_writer_resume(
        &mut self,
        failure: TypedFailure,
        holder: &str,
        fence: u64,
        at: String,
    ) -> Result<(), SandboxModelError> {
        self.require_lifecycle_authority(holder, fence)?;
        let mut next = self.clone();
        next.renew_checkpoint_writer_authority()?;
        next.park_failed_runtime_update(failure, holder, fence, at)?;
        *self = next;
        Ok(())
    }

    fn renew_checkpoint_writer_authority(&mut self) -> Result<(), SandboxModelError> {
        let authority = match self.lifecycle.operation.as_mut() {
            Some(SandboxLifecycleOperation::Pause {
                phase,
                authority,
                checkpoint_epoch,
            }) if matches!(
                *phase,
                SandboxPausePhase::CaptureAndPublishCheckpoint
                    | SandboxPausePhase::PauseAgents
                    | SandboxPausePhase::TeardownRuntime
            ) =>
            {
                *phase = SandboxPausePhase::CaptureAndPublishCheckpoint;
                *checkpoint_epoch = None;
                authority
            }
            Some(SandboxLifecycleOperation::ImageRotation {
                phase,
                authority,
                checkpoint_epoch,
                ..
            }) if matches!(
                *phase,
                SandboxImageRotationPhase::CaptureAndPublishCheckpoint
                    | SandboxImageRotationPhase::PauseAgents
                    | SandboxImageRotationPhase::TeardownRuntime
            ) =>
            {
                *phase = SandboxImageRotationPhase::CaptureAndPublishCheckpoint;
                *checkpoint_epoch = None;
                authority
            }
            _ => {
                return Err(SandboxModelError::invalid_value(
                    "sandbox.checkpoint_writer_recovery",
                ))
            }
        };
        authority.writer_resume_generation = authority
            .writer_resume_generation
            .checked_add(1)
            .ok_or_else(|| {
                SandboxModelError::invalid_value("sandbox.checkpoint_writer_recovery")
            })?;
        authority.writer_resume_token = Some(authority.expected_writer_resume_token());
        Ok(())
    }
}
