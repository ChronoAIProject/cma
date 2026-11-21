//! Exact retained-home authority selected before replacement runtime I/O.

use serde::{Deserialize, Serialize};

use super::{Sandbox, SandboxLifecycleState, SandboxModelError};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxHomeRecovery {
    pub home_identity: String,
    pub source_generation: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_pause: Option<SandboxRecoveryPause>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxRecoveryPause {
    pub notice: Option<String>,
}

impl SandboxLifecycleState {
    pub(super) fn select_home_recovery(
        &self,
    ) -> Result<Option<Box<SandboxHomeRecovery>>, SandboxModelError> {
        let Some(home) = &self.home else {
            return Ok(None);
        };
        let attachment = home.attachment().ok_or_else(invalid)?;
        if self.runtime_generation_high_water == 0 {
            return Err(invalid());
        }
        Ok(Some(Box::new(SandboxHomeRecovery {
            home_identity: attachment.identity().map_err(|_| invalid())?,
            source_generation: self.runtime_generation_high_water,
            resume_pause: None,
        })))
    }
}

impl SandboxHomeRecovery {
    pub fn validate_for(&self, sandbox: &Sandbox) -> Result<(), SandboxModelError> {
        let attachment = sandbox
            .lifecycle
            .home
            .as_ref()
            .and_then(|home| home.attachment())
            .ok_or_else(invalid)?;
        if self.home_identity != attachment.identity().map_err(|_| invalid())?
            || self.source_generation == 0
            || self.source_generation > sandbox.lifecycle.runtime_generation_high_water
            || self.resume_pause.as_ref().is_some_and(|pause| {
                pause.notice.as_ref().is_some_and(|notice| {
                    notice.trim().is_empty()
                        || notice.len() > 512
                        || notice.chars().any(char::is_control)
                })
            })
        {
            return Err(invalid());
        }
        Ok(())
    }
}

impl Sandbox {
    /// New owner work supersedes a pause interrupted by runtime loss. Persist this
    /// with the admission so a recovery worker cannot publish that obsolete pause.
    pub fn cancel_recovery_pause(&mut self, at: &str) -> Result<bool, SandboxModelError> {
        self.validate()?;
        if self
            .retained_home_recovery()
            .is_none_or(|source| source.resume_pause.is_none())
        {
            return Ok(false);
        }
        if crate::seconds_from_rfc3339(at).is_none()
            || crate::seconds_from_rfc3339(at) < crate::seconds_from_rfc3339(&self.updated_at)
        {
            return Err(invalid());
        }
        let mut next = self.clone();
        next.lifecycle
            .home_recovery
            .as_mut()
            .ok_or_else(invalid)?
            .resume_pause = None;
        next.revision = next.revision.checked_add(1).ok_or_else(invalid)?;
        next.updated_at = at.into();
        next.last_activity_at = at.into();
        next.validate()?;
        *self = next;
        Ok(true)
    }

    pub fn retained_home_recovery(&self) -> Option<&SandboxHomeRecovery> {
        self.lifecycle.home_recovery.as_deref()
    }

    pub fn complete_lifecycle_home_verification(
        &mut self,
        verified: &SandboxHomeRecovery,
        holder: &str,
        fence: u64,
        at: String,
    ) -> Result<(), SandboxModelError> {
        self.require_unpublished_runtime_quiescence()?;
        if self.retained_home_recovery() != Some(verified) {
            return Err(invalid());
        }
        verified.validate_for(self)?;
        self.advance_recovery_phase(
            super::SandboxRevivePhase::CheckpointVerification,
            super::SandboxRevivePhase::RuntimeProvisioning,
            super::SandboxImageRotationPhase::CheckpointVerification,
            super::SandboxImageRotationPhase::RuntimeProvisioning,
            holder,
            fence,
            at,
        )
    }
}

fn invalid() -> SandboxModelError {
    SandboxModelError::invalid_value("sandbox.home_recovery")
}

#[cfg(test)]
#[path = "home_recovery_tests.rs"]
mod tests;
