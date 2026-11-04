//! Retain failed allocation evidence without keeping a failed operation runnable.
use serde::{Deserialize, Serialize};

use super::{
    RuntimeBinding, Sandbox, SandboxHomeRecovery, SandboxLifecycleOperation, SandboxModelError,
    SandboxReviveCause, SandboxRevivePhase, SandboxRuntimeProvisionReservation, SandboxStatus,
    SandboxWorkStep,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxFailedRuntimeProvision {
    pub owner: crate::SubjectId,
    pub reservation: SandboxRuntimeProvisionReservation,
    pub operation: SandboxLifecycleOperation,
    pub source: RuntimeBinding,
    pub home: SandboxHomeRecovery,
}

impl Sandbox {
    /// This is custody of an exact prior attempt, never authority to replay it.
    pub(super) fn failed_provision_evidence(&self) -> Option<Box<SandboxFailedRuntimeProvision>> {
        let operation = self.lifecycle.operation.as_ref()?;
        let reservation = self.lifecycle.runtime_provision.as_ref()?;
        let source = self.lifecycle.suspended_runtime.as_deref()?;
        let home = self.retained_home_recovery()?;
        if self.runtime.is_some()
            || !matches!(
                operation,
                SandboxLifecycleOperation::Revive {
                    phase: SandboxRevivePhase::RuntimeProvisioning,
                    cause: SandboxReviveCause::RuntimeLost,
                    ..
                }
            )
            || reservation.operation_id != operation.authority().operation_id
            || reservation.generation != self.lifecycle.runtime_generation_high_water
            || source.provision_generation != Some(home.source_generation)
            || home.source_generation >= reservation.generation
        {
            return None;
        }
        Some(Box::new(SandboxFailedRuntimeProvision {
            owner: self.owner.clone(),
            reservation: reservation.clone(),
            operation: operation.clone(),
            source: source.clone(),
            home: home.clone(),
        }))
    }

    pub(super) fn failed_unpublished_provision(&self) -> bool {
        self.lifecycle
            .failed_provision
            .as_ref()
            .is_some_and(|evidence| evidence.validate_for(self).is_ok())
    }

    pub(super) fn validate_failed_provision_successor(
        &self,
        next: &Self,
    ) -> Result<(), SandboxModelError> {
        if self.lifecycle.failed_provision == next.lifecycle.failed_provision {
            return Ok(());
        }
        match (
            &self.lifecycle.failed_provision,
            &next.lifecycle.failed_provision,
        ) {
            (None, Some(evidence))
                if self.failed_provision_evidence().as_ref() == Some(evidence) =>
            {
                Ok(())
            }
            (Some(evidence), None)
                if matches!(
                    next.status,
                    SandboxStatus::Retiring | SandboxStatus::Retired
                ) || next
                    .lifecycle
                    .unpublished_recovery
                    .as_ref()
                    .is_some_and(|state| {
                        state.proof.runtime_id == evidence.source.runtime_id
                            && state.proof.runtime_generation == evidence.home.source_generation
                            && state.proof.home_identity == evidence.home.home_identity
                            && state
                                .proof
                                .reserved_provision
                                .as_ref()
                                .is_none_or(|reservation| reservation == &evidence.reservation)
                    }) =>
            {
                Ok(())
            }
            _ => Err(invalid()),
        }
    }
}

impl SandboxFailedRuntimeProvision {
    pub(super) fn validate_for(&self, sandbox: &Sandbox) -> Result<(), SandboxModelError> {
        self.home.validate_for(sandbox)?;
        let authority = self.operation.authority();
        if sandbox.status != SandboxStatus::Failed
            || self.owner != sandbox.owner
            || sandbox
                .failure
                .as_ref()
                .is_none_or(|failure| failure.class != "provisioning_failed")
            || sandbox.runtime.is_some()
            || sandbox.work.step != SandboxWorkStep::None
            || sandbox.lifecycle.operation.is_some()
            || sandbox.lifecycle.runtime_provision.is_some()
            || sandbox.lifecycle.home_recovery.is_some()
            || sandbox.lifecycle.suspended_runtime.as_deref() != Some(&self.source)
            || sandbox
                .lifecycle
                .home
                .as_ref()
                .is_none_or(|home| home.released)
            || sandbox.lifecycle.unpublished_recovery.is_some()
            || sandbox.lifecycle.unpublished_recovery_attempted
            || !matches!(
                self.operation,
                SandboxLifecycleOperation::Revive {
                    phase: SandboxRevivePhase::RuntimeProvisioning,
                    cause: SandboxReviveCause::RuntimeLost,
                    ..
                }
            )
            || self.reservation.operation_id != authority.operation_id
            || authority.generation == 0
            || crate::seconds_from_rfc3339(&authority.requested_at).is_none()
            || authority.writer_resume_token.is_some()
            || authority.writer_resume_generation != 0
            || authority.operation_id
                != format!(
                    "sandbox-lifecycle-v1:{}:{}",
                    sandbox.id, authority.generation
                )
            || authority.generation.checked_add(1) != Some(sandbox.lifecycle.next_authority)
            || self.reservation.generation != sandbox.lifecycle.runtime_generation_high_water
            || self.source.provision_generation != Some(self.home.source_generation)
            || self.home.source_generation >= self.reservation.generation
        {
            return Err(invalid());
        }
        Ok(())
    }
}

pub(super) fn invalid() -> SandboxModelError {
    SandboxModelError::invalid_value("sandbox.failed_runtime_provision")
}
