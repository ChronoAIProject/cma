//! Per-Sandbox image authority for explicitly approved custom-family migrations.
use layer3_adapter::HomeVolumeAttachment;
use serde::{Deserialize, Serialize};

use super::{Sandbox, SandboxModelError};
use crate::{SandboxId, SandboxProfileId, SubjectId};

mod admission;
mod recovery;
mod validation;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxCustomRuntimeOrigin {
    pub sandbox_id: SandboxId,
    pub owner: SubjectId,
    pub profile_id: SandboxProfileId,
    pub profile_revision: u32,
    pub profile_digest: String,
    pub profile_image: String,
    pub home: HomeVolumeAttachment,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxCustomRuntimeMigration {
    pub request_key: String,
    pub expected_revision: u64,
    pub expected_image: String,
    pub target_image: String,
    pub origin: SandboxCustomRuntimeOrigin,
}

/// The published pin changes only with verified lifecycle publication. The last
/// request remains after completion so a lost admission ACK cannot enqueue again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxCustomRuntimeState {
    pub effective_image: String,
    pub migration: SandboxCustomRuntimeMigration,
    pub source_generation: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub admission_basis: Option<Box<super::store::RuntimeMigrationAdmissionBasis>>,
}

impl SandboxCustomRuntimeOrigin {
    pub fn capture(sandbox: &Sandbox) -> Result<Self, SandboxModelError> {
        sandbox.validate()?;
        if sandbox.has_managed_runtime_image() {
            return Err(invalid());
        }
        let origin = Self {
            sandbox_id: sandbox.id.clone(),
            owner: sandbox.owner.clone(),
            profile_id: sandbox.profile.id.clone(),
            profile_revision: sandbox.profile.revision,
            profile_digest: sandbox.profile.digest.clone(),
            profile_image: sandbox
                .profile
                .content
                .runtime_image
                .clone()
                .ok_or_else(invalid)?,
            home: sandbox
                .lifecycle
                .home
                .as_ref()
                .and_then(|home| home.attachment())
                .ok_or_else(invalid)?,
        };
        origin.validate_for(sandbox)?;
        Ok(origin)
    }
}

impl Sandbox {
    /// Callers handling persisted data must validate before using this fallback.
    /// A migration never rewrites the immutable profile's original custom pin.
    pub fn effective_custom_runtime_image(&self) -> Option<&str> {
        if self.has_managed_desktop_image() {
            return None;
        }
        self.lifecycle
            .custom_runtime
            .as_ref()
            .map(|state| state.effective_image.as_str())
            .or(self.profile.content.runtime_image.as_deref())
    }

    /// Source inspection uses the old image until the successor binding exists.
    /// The bound successor can be inspected before its final publication without
    /// changing the permanent pin or treating a queued target as source authority.
    pub fn custom_runtime_image_for_binding(&self) -> Result<Option<&str>, SandboxModelError> {
        self.validate_custom_runtime()?;
        if self.has_managed_desktop_image() {
            return Ok(self
                .retained_runtime_binding()
                .map(|binding| binding.runtime_version.as_str())
                .or(self.profile.content.runtime_image.as_deref()));
        }
        let Some(state) = self.lifecycle.custom_runtime.as_ref() else {
            return Ok(self.profile.content.runtime_image.as_deref());
        };
        Ok(Some(
            self.retained_runtime_binding()
                .map(|binding| binding.runtime_version.as_str())
                .unwrap_or(&state.effective_image),
        ))
    }

    pub(super) fn validate_custom_rotation(&self, target: &str) -> Result<(), SandboxModelError> {
        if self.has_managed_runtime_image() {
            return Ok(());
        }
        self.validate_custom_runtime()?;
        let state = self.lifecycle.custom_runtime.as_ref().ok_or_else(invalid)?;
        if state.completed_at.is_some() || state.migration.target_image != target {
            return Err(invalid());
        }
        Ok(())
    }

    pub(super) fn complete_runtime_update_publication(
        &mut self,
        at: &str,
    ) -> Result<(), SandboxModelError> {
        let Some(target) = self.admitted_runtime_update_target().map(str::to_owned) else {
            return Ok(());
        };
        if self
            .runtime
            .as_ref()
            .is_none_or(|binding| binding.runtime_version != target)
        {
            return Err(invalid());
        }
        if let Some(transition) = self.admitted_retained_runtime_transition() {
            let matching = self
                .lifecycle
                .retained_startup_policy
                .as_ref()
                .zip(self.runtime.as_ref())
                .is_some_and(|(evidence, binding)| {
                    evidence.observation.legacy_journal_recovery_enabled
                        == transition.request.journal_recovery
                        && evidence.observation.runtime.generation
                            == transition.successor_generation
                        && binding.boot_id.as_deref()
                            == Some(evidence.observation.runtime.boot_id.as_str())
                });
            if !matching {
                return Err(invalid());
            }
        }
        self.validate_custom_runtime()?;
        if let Some(state) = self
            .lifecycle
            .custom_runtime
            .as_mut()
            .filter(|state| state.completed_at.is_none())
        {
            if state.migration.target_image != target {
                return Err(invalid());
            }
            state.effective_image = target;
            state.completed_at = Some(at.into());
        }
        if let Some(request) = self
            .lifecycle
            .runtime_update
            .as_mut()
            .and_then(|state| state.request.as_mut())
        {
            if self
                .runtime
                .as_ref()
                .is_none_or(|binding| binding.runtime_version != request.target_version)
            {
                return Err(invalid());
            }
            request.completed_at = Some(at.into());
            request.failure = None;
        }
        Ok(())
    }
}

fn invalid() -> SandboxModelError {
    SandboxModelError::invalid_value("sandbox.custom_runtime")
}

#[cfg(test)]
mod recovery_tests;
#[cfg(test)]
mod refusal_tests;
#[cfg(test)]
pub(in crate::sandbox) mod test_support;
#[cfg(test)]
mod tests;
