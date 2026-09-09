use crate::seconds_from_rfc3339;

use super::{
    RuntimeBinding, Sandbox, SandboxImageRotationPhase, SandboxLifecycleOperation,
    SandboxModelError, SandboxReviveFailureClass, SandboxRevivePhase, SandboxWorkStep,
};

const MAX_RUNTIME_ID_BYTES: usize = 512;
const MAX_RUNTIME_VERSION_BYTES: usize = 512;

impl Sandbox {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn advance_recovery_phase(
        &mut self,
        revive_current: SandboxRevivePhase,
        revive_next: SandboxRevivePhase,
        rotation_current: SandboxImageRotationPhase,
        rotation_next: SandboxImageRotationPhase,
        holder: &str,
        fence: u64,
        at: String,
    ) -> Result<(), SandboxModelError> {
        self.apply_claimed(holder, fence, at.clone(), at, |next| {
            match next.lifecycle.operation.as_mut() {
                Some(SandboxLifecycleOperation::Revive { phase, .. })
                    if *phase == revive_current =>
                {
                    *phase = revive_next;
                }
                Some(SandboxLifecycleOperation::ImageRotation { phase, .. })
                    if *phase == rotation_current =>
                {
                    *phase = rotation_next;
                }
                _ => return Err(invalid_execution()),
            }
            Ok(())
        })
    }

    pub(super) fn apply_claimed<T>(
        &mut self,
        holder: &str,
        fence: u64,
        ready_at: String,
        at: String,
        mutate: impl FnOnce(&mut Sandbox) -> Result<T, SandboxModelError>,
    ) -> Result<T, SandboxModelError> {
        self.apply_claimed_with_step(
            holder,
            fence,
            SandboxWorkStep::RunLifecycleOperation,
            ready_at,
            at,
            mutate,
        )
    }

    pub(super) fn apply_claimed_with_step<T>(
        &mut self,
        holder: &str,
        fence: u64,
        next_step: SandboxWorkStep,
        ready_at: String,
        at: String,
        mutate: impl FnOnce(&mut Sandbox) -> Result<T, SandboxModelError>,
    ) -> Result<T, SandboxModelError> {
        self.require_lifecycle_authority(holder, fence)?;
        if seconds_from_rfc3339(&ready_at).is_none() || seconds_from_rfc3339(&at).is_none() {
            return Err(invalid_execution());
        }
        let expected_revision = self.revision.checked_add(1).ok_or_else(invalid_execution)?;
        let mut next = self.clone();
        let result = mutate(&mut next)?;
        next.work.advance(holder, fence, next_step, ready_at)?;
        if next.revision == self.revision {
            next.revision = expected_revision;
        } else if next.revision != expected_revision {
            return Err(invalid_execution());
        }
        next.updated_at = at;
        next.validate()?;
        *self = next;
        Ok(result)
    }

    pub(super) fn require_lifecycle_claim(&self) -> Result<(), SandboxModelError> {
        self.validate()?;
        if self.work.step != SandboxWorkStep::RunLifecycleOperation || self.work.lease.is_none() {
            return Err(invalid_execution());
        }
        Ok(())
    }

    pub(super) fn require_lifecycle_authority(
        &self,
        holder: &str,
        fence: u64,
    ) -> Result<(), SandboxModelError> {
        self.require_lifecycle_claim()?;
        if !self.work.owns(holder, fence) {
            return Err(invalid_execution());
        }
        Ok(())
    }
}

pub(super) fn queued_message_phase(operation: Option<&SandboxLifecycleOperation>) -> bool {
    matches!(
        operation,
        Some(SandboxLifecycleOperation::Revive {
            phase: SandboxRevivePhase::QueuedMessageDelivery,
            ..
        }) | Some(SandboxLifecycleOperation::ImageRotation {
            phase: SandboxImageRotationPhase::QueuedMessageDelivery,
            ..
        })
    )
}

pub(super) fn lifecycle_failure_class(
    operation: &SandboxLifecycleOperation,
) -> SandboxReviveFailureClass {
    match operation {
        SandboxLifecycleOperation::Pause { .. } => SandboxReviveFailureClass::RestoreFailed,
        SandboxLifecycleOperation::Revive { phase, .. } => phase.failure_class(),
        SandboxLifecycleOperation::ImageRotation { phase, .. } => match phase {
            SandboxImageRotationPhase::RuntimeProvisioning
            | SandboxImageRotationPhase::CredentialDelivery => {
                SandboxReviveFailureClass::ProvisioningFailed
            }
            _ => SandboxReviveFailureClass::RestoreFailed,
        },
    }
}

pub(super) fn valid_runtime_binding(binding: &RuntimeBinding, at: &str) -> bool {
    binding.valid_boot_id()
        && binding.runtime_id.trim() == binding.runtime_id
        && !binding.runtime_id.is_empty()
        && binding.runtime_id.len() <= MAX_RUNTIME_ID_BYTES
        && !binding.runtime_id.chars().any(char::is_control)
        && binding.runtime_version.trim() == binding.runtime_version
        && !binding.runtime_version.is_empty()
        && binding.runtime_version.len() <= MAX_RUNTIME_VERSION_BYTES
        && !binding.runtime_version.chars().any(char::is_control)
        && binding.bound_at == at
        && seconds_from_rfc3339(&binding.bound_at).is_some()
}

pub(super) const fn invalid_execution() -> SandboxModelError {
    SandboxModelError::invalid_value("sandbox.lifecycle.execution")
}
