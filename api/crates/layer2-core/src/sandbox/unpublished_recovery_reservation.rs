//! Preserve an exact failed allocation separately from its retained source.
use super::{
    Sandbox, SandboxModelError, SandboxRuntimeProvisionReservation, SandboxStoreError,
    SandboxUnpublishedRecoveryProof, SandboxUnpublishedRecoverySnapshot,
    SandboxUnpublishedRecoveryState,
};

impl SandboxUnpublishedRecoverySnapshot {
    /// Only the complete empty-child stalled-loss snapshot may inspect this
    /// additional provider authority. A high-water number alone grants nothing.
    pub fn recovery_provision_reservation(
        &self,
    ) -> Result<Option<&SandboxRuntimeProvisionReservation>, SandboxStoreError> {
        self.validate()?;
        Ok(if self.sandbox.failed_unpublished_provision() {
            self.sandbox
                .lifecycle
                .failed_provision
                .as_ref()
                .map(|failed| &failed.reservation)
        } else {
            self.sandbox
                .stalled_unpublished_revival()
                .then_some(self.sandbox.lifecycle.runtime_provision.as_ref())
                .flatten()
        })
    }
}

impl SandboxUnpublishedRecoveryProof {
    pub(super) fn validate_reserved_provision(
        &self,
        snapshot: &SandboxUnpublishedRecoverySnapshot,
    ) -> Result<(), SandboxStoreError> {
        self.validate_observed_provision(snapshot)?;
        if let Some(reservation) = &self.reserved_provision {
            if snapshot.recovery_provision_reservation()? != Some(reservation)
                || !snapshot.agents.is_empty()
                || self.boot_id.is_some()
                || !self.runtime_process_absent
                || !self.conversations.is_empty()
                || reservation.generation <= self.runtime_generation
            {
                return Err(SandboxStoreError::InvalidTransaction(
                    "recovery allocation does not match its exact failed reservation",
                ));
            }
        }
        Ok(())
    }
}

impl SandboxUnpublishedRecoveryState {
    pub(super) fn validate_reserved_provision_for(
        &self,
        sandbox: &Sandbox,
    ) -> Result<(), SandboxModelError> {
        self.validate_observed_provision_for(sandbox)?;
        let Some(reservation) = &self.proof.reserved_provision else {
            return Ok(());
        };
        let operation = sandbox.lifecycle.operation.as_ref().ok_or_else(invalid)?;
        let prefix = format!("sandbox-lifecycle-v1:{}:", sandbox.id);
        let original_authority = reservation
            .operation_id
            .strip_prefix(&prefix)
            .and_then(|value| value.parse::<u64>().ok());
        // Admission validated the old operation before allocating this recovery
        // operation. Its immutable proof is retained even after the old pending
        // reservation is cleared and a later replacement reservation is made.
        if self.proof.boot_id.is_some()
            || !self.proof.runtime_process_absent
            || !self.proof.conversations.is_empty()
            || original_authority.is_none_or(|generation| {
                generation == 0 || generation >= operation.authority().generation
            })
            || reservation.generation <= self.proof.runtime_generation
            || reservation.generation > sandbox.lifecycle.runtime_generation_high_water
            || (self.quiescence.is_none()
                && reservation.generation != sandbox.lifecycle.runtime_generation_high_water)
        {
            return Err(invalid());
        }
        Ok(())
    }
}

const fn invalid() -> SandboxModelError {
    SandboxModelError::invalid_value("sandbox.unpublished_runtime_recovery.reserved_provision")
}
