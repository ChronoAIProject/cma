use super::{
    Sandbox, SandboxImageRotationPhase, SandboxLifecycleOperation, SandboxModelError,
    SandboxPausePhase,
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
        let authority = match next.lifecycle.operation.as_mut() {
            Some(SandboxLifecycleOperation::Pause {
                phase: SandboxPausePhase::CaptureAndPublishCheckpoint,
                authority,
                ..
            })
            | Some(SandboxLifecycleOperation::ImageRotation {
                phase: SandboxImageRotationPhase::CaptureAndPublishCheckpoint,
                authority,
                ..
            }) => authority,
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
        next.retry_lifecycle(ready_at, holder, fence, at)?;
        *self = next;
        Ok(())
    }
}
