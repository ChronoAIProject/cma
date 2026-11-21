//! A paused provider object remains durable authority until resumed or deleted.
use super::lifecycle_execution_support::{invalid_execution, valid_runtime_binding};
use super::{
    RuntimeBinding, Sandbox, SandboxImageRotationPhase, SandboxLifecycleOperation,
    SandboxModelError, SandboxPausePhase, SandboxStatus,
};

impl Sandbox {
    pub fn complete_lifecycle_runtime_suspension(
        &mut self,
        holder: &str,
        fence: u64,
        at: String,
    ) -> Result<(), SandboxModelError> {
        if self.runtime.is_none() || self.retained_home_recovery().is_none() {
            return Err(invalid_execution());
        }
        self.apply_claimed(holder, fence, at.clone(), at, |next| {
            match next.lifecycle.operation.as_mut() {
                Some(SandboxLifecycleOperation::Pause { phase, .. })
                    if *phase == SandboxPausePhase::TeardownRuntime =>
                {
                    *phase = SandboxPausePhase::PublishPaused;
                }
                Some(SandboxLifecycleOperation::ImageRotation { phase, .. })
                    if *phase == SandboxImageRotationPhase::TeardownRuntime =>
                {
                    *phase = SandboxImageRotationPhase::CheckpointVerification;
                }
                _ => return Err(invalid_execution()),
            }
            next.lifecycle.suspended_runtime = next.runtime.take().map(Box::new);
            Ok(())
        })
    }

    pub(super) fn validate_suspended_runtime(&self) -> Result<(), SandboxModelError> {
        let Some(binding) = self.lifecycle.suspended_runtime.as_deref() else {
            return Ok(());
        };
        if self.runtime.is_some()
            || !valid_runtime_binding(binding, &binding.bound_at)
            || binding.provision_generation.is_none_or(|generation| {
                generation == 0 || generation > self.lifecycle.runtime_generation_high_water
            })
            || self
                .lifecycle
                .home
                .as_ref()
                .is_none_or(|home| home.released || home.attachment().is_none())
            || !matches!(
                self.status,
                SandboxStatus::Pausing
                    | SandboxStatus::Paused
                    | SandboxStatus::Reviving
                    | SandboxStatus::Rotating
                    | SandboxStatus::Failed
                    | SandboxStatus::Retiring
            )
        {
            return Err(invalid_execution());
        }
        if self.status == SandboxStatus::Pausing
            && !matches!(
                self.lifecycle.operation,
                Some(SandboxLifecycleOperation::Pause {
                    phase: SandboxPausePhase::PublishPaused,
                    ..
                })
            )
        {
            return Err(invalid_execution());
        }
        Ok(())
    }

    pub(super) fn validate_resumed_binding(
        &self,
        binding: &RuntimeBinding,
    ) -> Result<(), SandboxModelError> {
        let Some(previous) = self.lifecycle.suspended_runtime.as_deref() else {
            return Ok(());
        };
        if binding.runtime_id != previous.runtime_id
            || binding.runtime_version
                != self
                    .admitted_runtime_update_target()
                    .unwrap_or(&previous.runtime_version)
            || binding.provision_generation <= previous.provision_generation
            || binding.boot_id.is_none()
            || binding.boot_id == previous.boot_id
        {
            return Err(invalid_execution());
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "suspended_runtime_tests.rs"]
mod tests;
