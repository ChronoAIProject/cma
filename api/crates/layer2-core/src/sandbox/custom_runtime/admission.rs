use super::{invalid, SandboxCustomRuntimeMigration, SandboxCustomRuntimeState};
use crate::sandbox::{
    Sandbox, SandboxModelError, SandboxRuntimeUpdateRequest, SandboxRuntimeUpdateState,
    SandboxStatus, SandboxWorkStep,
};
use crate::seconds_from_rfc3339;

impl Sandbox {
    /// `approved_target` comes from the operator service's qualified image
    /// authority, never directly from the HTTP body. Activity/child/session
    /// admission and this mutation must share the store's transaction and CAS.
    pub fn request_custom_runtime_migration(
        &mut self,
        request: SandboxCustomRuntimeMigration,
        approved_target: &str,
        at: String,
    ) -> Result<bool, SandboxModelError> {
        self.validate()?;
        request.validate_for(self)?;
        if request.target_image != approved_target
            || seconds_from_rfc3339(&at).is_none()
            || seconds_from_rfc3339(&at) < seconds_from_rfc3339(&self.updated_at)
        {
            return Err(invalid());
        }
        if let Some(state) = self.lifecycle.custom_runtime.as_ref() {
            if state.migration.request_key == request.request_key {
                if state.migration != request {
                    return Err(invalid());
                }
                if state.completed_at.is_some() {
                    return Ok(false);
                }
                let update = self.runtime_update_request().ok_or_else(invalid)?;
                if update.failure.is_none() {
                    return Ok(false);
                }
                if self.work.lease.is_some()
                    || self.admitted_runtime_update_target() != Some(request.target_image.as_str())
                {
                    return Err(invalid());
                }
                let mut next = self.clone();
                next.lifecycle
                    .runtime_update
                    .as_mut()
                    .and_then(|state| state.request.as_mut())
                    .ok_or_else(invalid)?
                    .failure = None;
                next.work
                    .schedule(SandboxWorkStep::RunLifecycleOperation, at.clone());
                next.touch_custom_runtime(at)?;
                *self = next;
                return Ok(true);
            }
            if state.completed_at.is_none() || state.migration.origin != request.origin {
                return Err(invalid());
            }
        }
        if self.revision != request.expected_revision
            || self.effective_custom_runtime_image() != Some(request.expected_image.as_str())
            || !matches!(self.status, SandboxStatus::Active | SandboxStatus::Paused)
            || self.lifecycle.operation.is_some()
            || self.lifecycle.checkpoint_capture.is_some()
            || self.lifecycle.runtime_provision.is_some()
            || self.lifecycle.unpublished_recovery.is_some()
            || self.work.lease.is_some()
            || !matches!(
                self.work.step,
                SandboxWorkStep::None | SandboxWorkStep::WaitForCredentialRotation
            )
            || self
                .runtime_update_request()
                .is_some_and(|update| update.completed_at.is_none())
        {
            return Err(invalid());
        }
        let home = self.lifecycle.home.as_ref().ok_or_else(invalid)?;
        let binding = self.retained_runtime_binding().ok_or_else(invalid)?;
        if home.released
            || home
                .native_pool
                .as_ref()
                .is_none_or(|native| native.cleaned || native.cleanup.is_some())
            || binding.runtime_version != request.expected_image
        {
            return Err(invalid());
        }
        if request.expected_image == request.target_image {
            return Ok(false);
        }
        let source_generation = binding
            .provision_generation
            .filter(|generation| *generation > 0)
            .ok_or_else(invalid)?;
        let mut next = self.clone();
        next.lifecycle.runtime_update = Some(SandboxRuntimeUpdateState {
            checked_at: at.clone(),
            observed_version: request.target_image.clone(),
            request: Some(SandboxRuntimeUpdateRequest {
                target_version: request.target_image.clone(),
                requested_at: at.clone(),
                automatic: false,
                started_at: None,
                completed_at: None,
                failure: None,
                retained_transition: None,
            }),
        });
        next.lifecycle.custom_runtime = Some(Box::new(SandboxCustomRuntimeState {
            effective_image: request.expected_image.clone(),
            migration: request,
            source_generation,
            completed_at: None,
            admission_basis: None,
        }));
        next.touch_custom_runtime(at)?;
        tracing::info!(
            revision = next.revision,
            source_generation,
            "Queued approved custom runtime migration"
        );
        *self = next;
        Ok(true)
    }

    fn touch_custom_runtime(&mut self, at: String) -> Result<(), SandboxModelError> {
        self.revision = self.revision.checked_add(1).ok_or_else(invalid)?;
        self.updated_at = at;
        self.validate()
    }
}
