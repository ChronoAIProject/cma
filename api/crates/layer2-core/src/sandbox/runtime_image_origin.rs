//! The authority that selected the immutable image used at creation.
use serde::{Deserialize, Serialize};

use super::{Sandbox, SandboxModelError, SandboxOrigin, SandboxStatus};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxRuntimeImageOrigin {
    ManagedDesktop,
    UserSelected,
}

impl Sandbox {
    pub fn has_managed_runtime_image(&self) -> bool {
        self.profile.content.runtime_image.is_none() || self.has_managed_desktop_image()
    }

    pub fn has_managed_desktop_image(&self) -> bool {
        self.lifecycle.runtime_image_origin == Some(SandboxRuntimeImageOrigin::ManagedDesktop)
    }

    /// A retained version can be displayed even when it cannot establish the
    /// current runtime status after an interruption or during a transition.
    pub fn managed_desktop_runtime_confirmed(&self) -> bool {
        !self.has_managed_desktop_image()
            || (matches!(self.status, SandboxStatus::Active | SandboxStatus::Paused)
                && self.lifecycle.interruption.is_none()
                && self.retained_runtime_binding().is_some_and(|binding| {
                    binding.boot_id.is_some() && binding.provision_generation.is_some()
                }))
    }

    pub(super) fn validate_runtime_image_origin(&self) -> Result<(), SandboxModelError> {
        if self.lifecycle.runtime_image_origin.is_some()
            && (!matches!(self.origin, SandboxOrigin::Lifecycle { .. })
                || self
                    .profile
                    .content
                    .runtime_image
                    .as_deref()
                    .is_none_or(str::is_empty)
                || (self.has_managed_desktop_image() && self.lifecycle.custom_runtime.is_some()))
        {
            return Err(SandboxModelError::invalid_value(
                "sandbox.runtime_image_origin",
            ));
        }
        Ok(())
    }
}
