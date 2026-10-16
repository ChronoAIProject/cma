//! Durable, bounded information explaining an unexpected runtime interruption.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxInterruptionCause {
    OutOfMemory,
    RuntimeRestarted,
    #[default]
    RuntimeUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxInterruption {
    pub cause: SandboxInterruptionCause,
    pub detected_at: String,
    pub memory_limit_mib: u32,
}

impl SandboxInterruption {
    pub(super) fn validate(&self) -> Result<(), super::SandboxModelError> {
        if crate::seconds_from_rfc3339(&self.detected_at).is_none() || self.memory_limit_mib == 0 {
            return Err(super::SandboxModelError::invalid_value(
                "sandbox.interruption",
            ));
        }
        Ok(())
    }
}
