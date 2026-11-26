//! The authority that selected the immutable image used at creation.
use serde::{Deserialize, Serialize};

use super::{Sandbox, SandboxModelError, SandboxOrigin, SandboxStatus};
use crate::released_runtime::ReleasedLane;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxRuntimeImageOrigin {
    ManagedDesktop,
    ManagedMachine,
    UserSelected,
}

impl Sandbox {
    pub fn has_managed_runtime_image(&self) -> bool {
        self.managed_runtime_lane().is_some()
    }

    pub fn managed_runtime_lane(&self) -> Option<ReleasedLane> {
        match self.lifecycle.runtime_image_origin {
            Some(SandboxRuntimeImageOrigin::ManagedMachine) => Some(ReleasedLane::Machine),
            Some(SandboxRuntimeImageOrigin::ManagedDesktop) => Some(ReleasedLane::Agent),
            None if self.profile.content.runtime_image.is_none() => Some(ReleasedLane::Agent),
            Some(SandboxRuntimeImageOrigin::UserSelected) | None => None,
        }
    }

    pub fn has_managed_desktop_image(&self) -> bool {
        matches!(
            self.lifecycle.runtime_image_origin,
            Some(
                SandboxRuntimeImageOrigin::ManagedDesktop
                    | SandboxRuntimeImageOrigin::ManagedMachine
            )
        )
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
