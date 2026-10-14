//! Observe one verified persistent runtime without disclosing credentials or file contents.

use crate::{AdapterError, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeStatusRequest {
    pub sandbox_id: String,
    pub generation: u64,
    pub home_identity: String,
}

impl RuntimeStatusRequest {
    pub fn validate(&self) -> Result<()> {
        if self.sandbox_id.is_empty()
            || self.sandbox_id.len() > 256
            || self.sandbox_id.trim() != self.sandbox_id
            || self.sandbox_id.chars().any(char::is_control)
            || self.generation == 0
            || !valid_home_identity(&self.home_identity)
        {
            return Err(AdapterError::Rejected(
                "runtime status identity is invalid".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeStatus {
    /// Absent on retained supervisors that predate the additive operation.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub agent_skill_install: bool,
    pub sandbox_id: String,
    pub generation: u64,
    pub home_identity: String,
    /// Changes on every supervisor process start, including a replacement behind the same handle.
    pub boot_id: String,
    pub installation_ready: bool,
    pub credential_ready: bool,
    pub restore_pending: bool,
    pub agent_processes: u16,
}

impl RuntimeStatus {
    pub fn validate_for(&self, request: &RuntimeStatusRequest) -> Result<()> {
        request.validate()?;
        if self.sandbox_id != request.sandbox_id
            || self.generation != request.generation
            || self.home_identity != request.home_identity
            || self.boot_id.len() != 32
            || !self
                .boot_id
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || (self.installation_ready && self.restore_pending)
            || (self.credential_ready && !self.installation_ready && !self.restore_pending)
            || (self.agent_processes != 0 && !self.installation_ready)
            || self.agent_processes > 16
        {
            return Err(AdapterError::Rejected(
                "runtime status does not prove the requested identity".into(),
            ));
        }
        Ok(())
    }
}

pub fn valid_home_identity(value: &str) -> bool {
    value.len() == 43
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
}
