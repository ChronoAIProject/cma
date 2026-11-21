//! Explicit recovery of a failed retained allocation whose prior operation was lost.
//! The ceiling bounds a separately proved allocation; it never recreates an operation.
use super::{
    Sandbox, SandboxModelError, SandboxStatus, SandboxStoreError, SandboxUnpublishedRecoveryProof,
    SandboxUnpublishedRecoverySnapshot, SandboxUnpublishedRecoveryState, SandboxWorkStep,
};

impl Sandbox {
    /// Preselection only. A trusted verifier must prove the exact current allocation,
    /// same home, and process absence; admission must fence the complete empty child set.
    pub fn legacy_unpublished_allocation(&self) -> bool {
        self.status == SandboxStatus::Failed
            && self.runtime.is_none()
            && self.work.step == SandboxWorkStep::None
            && self.work.lease.is_none()
            && self
                .failure
                .as_ref()
                .is_some_and(|f| f.class == "provisioning_failed")
            && self.lifecycle.failed_provision.is_none()
            && self.lifecycle.operation.is_none()
            && self.lifecycle.runtime_provision.is_none()
            && self.lifecycle.home_recovery.is_none()
            && self.lifecycle.checkpoint_capture.is_none()
            && self.lifecycle.unpublished_recovery.is_none()
            && !self.lifecycle.unpublished_recovery_attempted
            && self.lifecycle.next_authority > 1
            && self
                .lifecycle
                .suspended_runtime
                .as_ref()
                .is_some_and(|source| {
                    source.provision_generation.is_some_and(|generation| {
                        generation > 0 && generation < self.lifecycle.runtime_generation_high_water
                    })
                })
            && self.supports_retained_runtime_update()
    }

    pub(super) fn unpublished_allocation_source(&self) -> bool {
        self.stalled_unpublished_revival()
            || self.failed_unpublished_provision()
            || self.legacy_unpublished_allocation()
    }
}

impl SandboxUnpublishedRecoveryProof {
    pub(super) fn validate_observed_provision(
        &self,
        snapshot: &SandboxUnpublishedRecoverySnapshot,
    ) -> Result<(), SandboxStoreError> {
        if snapshot.sandbox.legacy_unpublished_allocation() {
            if self.observed_provision_generation
                != Some(snapshot.sandbox.lifecycle.runtime_generation_high_water)
                || self.reserved_provision.is_some()
                || !snapshot.agents.is_empty()
                || self.boot_id.is_some()
                || !self.runtime_process_absent
                || !self.conversations.is_empty()
            {
                return Err(refused());
            }
        } else if self.observed_provision_generation.is_some() {
            return Err(refused());
        }
        Ok(())
    }
}

impl SandboxUnpublishedRecoveryState {
    pub(super) fn validate_observed_provision_for(
        &self,
        sandbox: &Sandbox,
    ) -> Result<(), SandboxModelError> {
        if let Some(generation) = self.proof.observed_provision_generation {
            if self.proof.reserved_provision.is_some()
                || self.proof.boot_id.is_some()
                || !self.proof.runtime_process_absent
                || !self.proof.conversations.is_empty()
                || generation <= self.proof.runtime_generation
                || generation > sandbox.lifecycle.runtime_generation_high_water
                || (self.quiescence.is_none()
                    && generation != sandbox.lifecycle.runtime_generation_high_water)
            {
                return Err(SandboxModelError::invalid_value(
                    "sandbox.unpublished_runtime_recovery.observed_provision_generation",
                ));
            }
        }
        Ok(())
    }
}

fn refused() -> SandboxStoreError {
    SandboxStoreError::InvalidTransaction("lost allocation requires independent current authority")
}
