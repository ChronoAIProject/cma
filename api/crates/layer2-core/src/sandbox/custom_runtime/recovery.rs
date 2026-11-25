use super::invalid;
use crate::sandbox::{Sandbox, SandboxModelError};

impl Sandbox {
    /// Recovery reuses recorded custom authority; this never admits a first
    /// migration or changes ordinary managed/default update eligibility.
    pub fn supports_retained_custom_runtime_recovery(&self) -> bool {
        self.lifecycle.custom_runtime.is_some()
            && self.validate_custom_runtime().is_ok()
            && self.retained_runtime_binding().is_some()
            && self.lifecycle.home.as_ref().is_some_and(|home| {
                !home.released
                    && home
                        .native_pool
                        .as_ref()
                        .is_some_and(|native| !native.cleaned && native.cleanup.is_none())
            })
    }

    pub(in crate::sandbox) fn validate_custom_recovery_target(
        &self,
        target: &str,
    ) -> Result<(), SandboxModelError> {
        if self.has_managed_runtime_image() {
            return Ok(());
        }
        self.validate_custom_runtime()?;
        let state = self.lifecycle.custom_runtime.as_ref().ok_or_else(invalid)?;
        let expected = if state.completed_at.is_none() {
            &state.migration.target_image
        } else {
            &state.effective_image
        };
        if target != expected {
            return Err(invalid());
        }
        Ok(())
    }

    pub(in crate::sandbox) fn preserve_custom_recovery_request(
        &mut self,
        previous: &Sandbox,
    ) -> Result<(), SandboxModelError> {
        if previous
            .lifecycle
            .custom_runtime
            .as_ref()
            .is_none_or(|state| state.completed_at.is_some())
        {
            return Ok(());
        }
        // A worker's automatic repair is still executing the original operator
        // migration, not issuing new automatic permission to change its image.
        let original = previous.runtime_update_request().ok_or_else(invalid)?;
        let request = self
            .lifecycle
            .runtime_update
            .as_mut()
            .and_then(|state| state.request.as_mut())
            .ok_or_else(invalid)?;
        request.requested_at.clone_from(&original.requested_at);
        request.automatic = original.automatic;
        if original.started_at.is_some() {
            request.started_at.clone_from(&original.started_at);
        }
        Ok(())
    }
}
