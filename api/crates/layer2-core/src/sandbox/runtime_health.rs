//! Bind observed process identity to durable Sandbox and home authority.

use layer3_adapter::{RuntimeStatus, RuntimeStatusRequest};

use super::{RuntimeBinding, Sandbox, SandboxModelError};

impl RuntimeBinding {
    pub fn valid_boot_id(&self) -> bool {
        self.boot_id.as_ref().is_none_or(|boot| {
            self.provision_generation.is_some()
                && boot.len() == 32
                && boot
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
    }
}

impl Sandbox {
    /// Refuse foreign observations; false identifies a replaced supervisor.
    pub fn runtime_process_matches(
        &self,
        observed: &RuntimeStatus,
    ) -> Result<bool, SandboxModelError> {
        self.validate()?;
        if self.credential.delivered && !observed.credential_ready && observed.agent_processes != 0
        {
            return Err(invalid());
        }
        let binding = self.runtime.as_ref().ok_or_else(invalid)?;
        let home = self
            .lifecycle
            .home
            .as_ref()
            .and_then(|home| home.attachment())
            .ok_or_else(invalid)?;
        observed
            .validate_for(&RuntimeStatusRequest {
                sandbox_id: self.id.to_string(),
                generation: binding.provision_generation.ok_or_else(invalid)?,
                home_identity: home.identity().map_err(|_| invalid())?,
            })
            .map_err(|_| invalid())?;
        Ok(binding
            .boot_id
            .as_ref()
            .is_none_or(|boot| boot == &observed.boot_id))
    }

    /// Adopt an initial observation once, under the caller's exact DB revision.
    /// A newer observation can never overwrite an already recorded process.
    pub fn record_runtime_boot(
        &mut self,
        expected: &RuntimeBinding,
        observed: &RuntimeStatus,
        at: String,
    ) -> Result<(), SandboxModelError> {
        if self.runtime.as_ref() != Some(expected)
            || !self.runtime_process_matches(observed)?
            || crate::seconds_from_rfc3339(&at).is_none()
            || crate::seconds_from_rfc3339(&at) < crate::seconds_from_rfc3339(&self.updated_at)
        {
            return Err(invalid());
        }
        if expected.boot_id.is_some() {
            return Ok(());
        }
        let mut next = self.clone();
        next.runtime.as_mut().ok_or_else(invalid)?.boot_id = Some(observed.boot_id.clone());
        next.revision = next.revision.checked_add(1).ok_or_else(invalid)?;
        next.updated_at = at;
        next.validate()?;
        *self = next;
        Ok(())
    }
}

const fn invalid() -> SandboxModelError {
    SandboxModelError::invalid_value("sandbox.runtime_process")
}

#[cfg(test)]
#[path = "runtime_health_tests.rs"]
mod tests;
