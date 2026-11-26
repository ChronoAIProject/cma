use super::*;
use crate::sandbox::{
    lifecycle_execution_support::valid_runtime_binding, RetainedRuntimePolicyBinding,
    RuntimeBinding,
};

impl Sandbox {
    /// Bind the generation-specific replacement after restore bootstrap succeeds.
    pub fn bind_lifecycle_runtime(
        &mut self,
        binding: RuntimeBinding,
        holder: &str,
        fence: u64,
        at: String,
    ) -> Result<(), SandboxModelError> {
        if self.admitted_retained_runtime_transition().is_some() {
            return Err(invalid_execution());
        }
        self.bind_lifecycle_runtime_inner(binding, None, holder, fence, at)
    }

    /// Credit the negotiated policy only on the exact admitted binding.
    pub fn bind_retained_lifecycle_runtime(
        &mut self,
        binding: RuntimeBinding,
        observation: layer3_adapter::RuntimeStartupPolicyObservation,
        holder: &str,
        fence: u64,
        at: String,
    ) -> Result<(), SandboxModelError> {
        let admitted = self
            .admitted_retained_runtime_transition()
            .ok_or_else(invalid_execution)?;
        observation
            .runtime
            .validate_for(&layer3_adapter::RuntimeStatusRequest {
                sandbox_id: self.id.to_string(),
                generation: admitted.successor_generation,
                home_identity: admitted.request.predecessor.home_identity.clone(),
            })
            .map_err(|_| invalid_execution())?;
        if observation.legacy_journal_recovery_enabled != admitted.request.journal_recovery
            || binding.runtime_version != admitted.request.target_image
            || binding.boot_id.as_deref() != Some(observation.runtime.boot_id.as_str())
            || !observation.runtime.installation_ready
            || observation.runtime.restore_pending
            || observation.runtime.credential_ready
            || observation.runtime.agent_processes != 0
        {
            return Err(invalid_execution());
        }
        self.bind_lifecycle_runtime_inner(binding, Some(observation), holder, fence, at)
    }

    fn bind_lifecycle_runtime_inner(
        &mut self,
        binding: RuntimeBinding,
        observation: Option<layer3_adapter::RuntimeStartupPolicyObservation>,
        holder: &str,
        fence: u64,
        at: String,
    ) -> Result<(), SandboxModelError> {
        self.validate_resumed_binding(&binding)?;
        if self.runtime.is_some()
            || !valid_runtime_binding(&binding, &at)
            || self
                .lifecycle
                .runtime_provision
                .as_ref()
                .is_none_or(|reservation| {
                    binding.provision_generation != Some(reservation.generation)
                })
        {
            return Err(invalid_execution());
        }
        self.apply_claimed(holder, fence, at.clone(), at, |next| {
            match next.lifecycle.operation.as_mut() {
                Some(SandboxLifecycleOperation::Revive { phase, .. })
                    if *phase == SandboxRevivePhase::RuntimeProvisioning =>
                {
                    *phase = SandboxRevivePhase::CredentialDelivery;
                }
                Some(SandboxLifecycleOperation::ImageRotation {
                    phase,
                    target_runtime_version,
                    ..
                }) if *phase == SandboxImageRotationPhase::RuntimeProvisioning
                    && *target_runtime_version == binding.runtime_version =>
                {
                    *phase = SandboxImageRotationPhase::CredentialDelivery;
                }
                _ => return Err(invalid_execution()),
            }
            next.runtime = Some(binding);
            if let Some(observation) = observation {
                next.lifecycle.retained_startup_policy =
                    Some(Box::new(RetainedRuntimePolicyBinding { observation }));
            }
            if next.lifecycle.unpublished_recovery.is_some() {
                next.require_unpublished_runtime_quiescence()?;
                next.lifecycle.unpublished_recovery = None;
                next.lifecycle.unpublished_recovery_attempted = true;
            }
            next.lifecycle.suspended_runtime = None;
            next.lifecycle.runtime_provision = None;
            Ok(())
        })
    }
}
