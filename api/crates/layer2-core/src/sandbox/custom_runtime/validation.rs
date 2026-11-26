use super::{invalid, SandboxCustomRuntimeMigration, SandboxCustomRuntimeOrigin};
use crate::sandbox::{Sandbox, SandboxModelError, SandboxStatus};
use crate::seconds_from_rfc3339;

impl SandboxCustomRuntimeOrigin {
    pub(super) fn validate_for(&self, sandbox: &Sandbox) -> Result<(), SandboxModelError> {
        let home = sandbox.lifecycle.home.as_ref().ok_or_else(invalid)?;
        let native = home.native_pool.as_ref().ok_or_else(invalid)?;
        let authority = self
            .home
            .receipt
            .native_authority
            .as_ref()
            .ok_or_else(invalid)?;
        self.home.request.validate().map_err(|_| invalid())?;
        self.home
            .receipt
            .validate_for(&self.home.request)
            .map_err(|_| invalid())?;
        if self.sandbox_id != sandbox.id
            || self.owner != sandbox.owner
            || self.profile_id != sandbox.profile.id
            || self.profile_revision != sandbox.profile.revision
            || self.profile_digest != sandbox.profile.digest
            || Some(self.profile_image.as_str()) != sandbox.profile.content.runtime_image.as_deref()
            || repository(&self.profile_image).is_none()
            || self.home.request.sandbox_id != sandbox.id.as_str()
            || !self.home.request.policy.is_native_vct()
            || home.request != self.home.request
            || home.receipt.as_ref() != Some(&self.home.receipt)
            || native.request != authority.claim.request
            || native.claim.as_ref() != Some(&authority.claim)
        {
            return Err(invalid());
        }
        Ok(())
    }
}

impl SandboxCustomRuntimeMigration {
    pub(super) fn validate_for(&self, sandbox: &Sandbox) -> Result<(), SandboxModelError> {
        self.origin.validate_for(sandbox)?;
        if self.request_key.trim().is_empty()
            || self.request_key.len() > 256
            || self.request_key.chars().any(char::is_control)
            || self.expected_revision == 0
            || self.expected_revision > sandbox.revision
            || repository(&self.expected_image) != repository(&self.origin.profile_image)
            || repository(&self.target_image) != repository(&self.origin.profile_image)
        {
            return Err(invalid());
        }
        Ok(())
    }
}

impl Sandbox {
    pub(in crate::sandbox) fn validate_custom_runtime(&self) -> Result<(), SandboxModelError> {
        let Some(state) = self.lifecycle.custom_runtime.as_ref() else {
            return Ok(());
        };
        state.migration.validate_for(self)?;
        if state.source_generation == 0
            || state.source_generation > self.lifecycle.runtime_generation_high_water
            || state.migration.expected_image == state.migration.target_image
            || state
                .completed_at
                .as_deref()
                .is_some_and(|at| seconds_from_rfc3339(at).is_none())
            || state.effective_image
                != if state.completed_at.is_some() {
                    &state.migration.target_image
                } else {
                    &state.migration.expected_image
                }
                .as_str()
        {
            return Err(invalid());
        }
        let pending = state.completed_at.is_none();
        let request = self.runtime_update_request();
        if pending
            && request.is_none_or(|request| {
                request.automatic
                    || request.target_version != state.migration.target_image
                    || request.completed_at.is_some()
            })
        {
            return Err(invalid());
        }
        // Neither a later default nor an unrelated lifecycle operation may turn
        // retained custom authority into permission for another repository/pin.
        if self.admitted_runtime_update_target().is_some_and(|target| {
            target != state.effective_image && (!pending || target != state.migration.target_image)
        }) {
            return Err(invalid());
        }
        if let Some(binding) = self.retained_runtime_binding() {
            let authority = state
                .migration
                .origin
                .home
                .receipt
                .native_authority
                .as_ref()
                .ok_or_else(invalid)?;
            let name = authority
                .claim
                .sandbox_name
                .as_deref()
                .ok_or_else(invalid)?;
            if binding.runtime_id != format!("agent-sandbox:{name}:{}", authority.sandbox_uid) {
                return Err(invalid());
            }
            if binding.runtime_version != state.effective_image
                && !(pending
                    && binding.runtime_version == state.migration.target_image
                    && (self.admitted_runtime_update_target()
                        == Some(state.migration.target_image.as_str())
                        || matches!(
                            self.status,
                            SandboxStatus::Failed
                                | SandboxStatus::Retiring
                                | SandboxStatus::Retired
                        ))
                    && request.is_some_and(|request| request.started_at.is_some())
                    && binding
                        .provision_generation
                        .is_some_and(|generation| generation > state.source_generation))
            {
                return Err(invalid());
            }
        }
        Ok(())
    }
}

fn repository(image: &str) -> Option<&str> {
    if image.len() > 512 || !crate::profiles::valid_runtime_image(image) {
        return None;
    }
    let (repository, _) = image.rsplit_once("@sha256:")?;
    // Digests identify the artifact; a tag must not redefine the family. Ports
    // remain legal in the registry component, but tags in the last one do not.
    (!repository.rsplit('/').next()?.contains(':')).then_some(repository)
}
