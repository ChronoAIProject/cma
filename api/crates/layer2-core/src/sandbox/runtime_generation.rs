//! Persisted provider allocation authority, independent of live Agent membership.

use serde::{Deserialize, Serialize};

use super::{
    RuntimeBinding, Sandbox, SandboxImageRotationPhase, SandboxLifecycleOperation,
    SandboxModelError, SandboxRevivePhase,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxRuntimeProvisionReservation {
    pub operation_id: String,
    pub generation: u64,
}

impl Sandbox {
    /// Reserve before provider I/O, then commit and reclaim this same phase.
    /// Repeated requests under the same operation reuse the durable reservation.
    pub fn reserve_lifecycle_runtime_generation(
        &mut self,
        minimum_generation: u64,
        holder: &str,
        fence: u64,
        at: String,
    ) -> Result<u64, SandboxModelError> {
        self.require_lifecycle_authority(holder, fence)?;
        self.require_unpublished_runtime_quiescence()?;
        if !runtime_provisioning(self) || self.runtime.is_some() {
            return Err(invalid_generation());
        }
        if let Some(reservation) = self.lifecycle.runtime_provision.as_ref() {
            return Ok(reservation.generation);
        }
        let generation = if let Some(transition) = self.admitted_retained_runtime_transition() {
            if minimum_generation > transition.successor_generation {
                return Err(invalid_generation());
            }
            transition.successor_generation
        } else {
            self.lifecycle
                .runtime_generation_high_water
                .checked_add(1)
                .ok_or_else(invalid_generation)?
                .max(minimum_generation)
        };
        let operation_id = self
            .lifecycle
            .operation
            .as_ref()
            .ok_or_else(invalid_generation)?
            .authority()
            .operation_id
            .clone();
        self.apply_claimed(holder, fence, at.clone(), at, |next| {
            next.lifecycle.runtime_generation_high_water = generation;
            next.lifecycle.runtime_provision = Some(SandboxRuntimeProvisionReservation {
                operation_id,
                generation,
            });
            Ok(generation)
        })
    }

    /// Record a provider-observed generation only after exact binding adoption.
    pub fn record_adopted_runtime_generation(
        &mut self,
        expected_runtime: &RuntimeBinding,
        generation: u64,
        at: String,
    ) -> Result<(), SandboxModelError> {
        self.validate()?;
        if generation == 0
            || self.runtime.as_ref() != Some(expected_runtime)
            || crate::seconds_from_rfc3339(&at).is_none()
            || expected_runtime
                .provision_generation
                .is_some_and(|known| known != generation)
        {
            return Err(invalid_generation());
        }
        if expected_runtime.provision_generation == Some(generation) {
            return Ok(());
        }
        let mut next = self.clone();
        next.runtime
            .as_mut()
            .ok_or_else(invalid_generation)?
            .provision_generation = Some(generation);
        next.lifecycle.runtime_generation_high_water =
            next.lifecycle.runtime_generation_high_water.max(generation);
        next.revision = next
            .revision
            .checked_add(1)
            .ok_or_else(invalid_generation)?;
        next.updated_at = at;
        next.validate()?;
        *self = next;
        Ok(())
    }

    /// Generic persistence paths must never erase known provider authority.
    pub fn validate_runtime_generation_successor(
        &self,
        next: &Sandbox,
    ) -> Result<(), SandboxModelError> {
        self.validate_unpublished_recovery_successor(next)?;
        if self.lifecycle.runtime_image_origin != next.lifecycle.runtime_image_origin {
            return Err(SandboxModelError::invalid_value(
                "sandbox.runtime_image_origin",
            ));
        }
        next.validate_runtime_generation()?;
        if next.lifecycle.runtime_generation_high_water
            < self.lifecycle.runtime_generation_high_water
        {
            return Err(invalid_generation());
        }
        if let (Some(current), Some(binding)) = (&self.runtime, &next.runtime) {
            if current.runtime_id == binding.runtime_id
                && current.boot_id.is_some()
                && current.boot_id != binding.boot_id
            {
                return Err(invalid_generation());
            }
            if current.runtime_id == binding.runtime_id
                && current.provision_generation.is_some()
                && current.provision_generation != binding.provision_generation
            {
                return Err(invalid_generation());
            }
        }
        if let Some(binding) = next.runtime.as_ref() {
            let different_handle = self
                .runtime
                .as_ref()
                .is_none_or(|current| current.runtime_id != binding.runtime_id);
            if different_handle
                && match binding.provision_generation {
                    Some(generation) => generation < self.lifecycle.runtime_generation_high_water,
                    None => self.lifecycle.runtime_generation_high_water > 0,
                }
            {
                return Err(invalid_generation());
            }
        }
        Ok(())
    }

    pub(super) fn validate_runtime_generation(&self) -> Result<(), SandboxModelError> {
        let high_water = self.lifecycle.runtime_generation_high_water;
        if self.runtime.as_ref().is_some_and(|runtime| {
            !runtime.valid_boot_id()
                || (runtime.boot_id.is_some()
                    && self
                        .lifecycle
                        .home
                        .as_ref()
                        .and_then(|home| home.attachment())
                        .is_none())
        }) {
            return Err(invalid_generation());
        }
        if self
            .runtime
            .as_ref()
            .and_then(|runtime| runtime.provision_generation)
            .is_some_and(|generation| generation == 0 || generation > high_water)
        {
            return Err(invalid_generation());
        }
        if let Some(reservation) = self.lifecycle.runtime_provision.as_ref() {
            if !runtime_provisioning(self)
                || self.runtime.is_some()
                || reservation.generation == 0
                || reservation.generation != high_water
                || self.lifecycle.operation.as_ref().is_none_or(|operation| {
                    operation.authority().operation_id != reservation.operation_id
                })
            {
                return Err(invalid_generation());
            }
        }
        Ok(())
    }
}

fn runtime_provisioning(sandbox: &Sandbox) -> bool {
    matches!(
        sandbox.lifecycle.operation,
        Some(SandboxLifecycleOperation::Revive {
            phase: SandboxRevivePhase::RuntimeProvisioning,
            ..
        }) | Some(SandboxLifecycleOperation::ImageRotation {
            phase: SandboxImageRotationPhase::RuntimeProvisioning,
            ..
        })
    )
}

const fn invalid_generation() -> SandboxModelError {
    SandboxModelError::invalid_value("sandbox.runtime_generation")
}
