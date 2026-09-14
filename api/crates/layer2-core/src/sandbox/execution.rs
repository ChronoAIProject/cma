use serde::{Deserialize, Serialize};

use super::SandboxModelError;

/// Deployment execution policy pinned before a provisioning side effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProvisioningExecution {
    pub command_timeout_seconds: u64,
    pub total_timeout_seconds: u64,
    pub clone_depth: u32,
}

impl Default for ProvisioningExecution {
    fn default() -> Self {
        Self {
            command_timeout_seconds: 600,
            total_timeout_seconds: 1_800,
            clone_depth: 50,
        }
    }
}

impl ProvisioningExecution {
    pub fn validate(&self) -> Result<(), SandboxModelError> {
        if !(1..=600).contains(&self.command_timeout_seconds)
            || !(self.command_timeout_seconds..=1_800).contains(&self.total_timeout_seconds)
            || !(1..=1_000).contains(&self.clone_depth)
        {
            return Err(SandboxModelError::invalid_value("provisioning.execution"));
        }
        Ok(())
    }

    pub fn validate_successor(
        current: Option<Self>,
        next: Option<Self>,
    ) -> Result<(), SandboxModelError> {
        if current.is_some() && current != next {
            return Err(SandboxModelError::invalid_value("provisioning.execution"));
        }
        next.map_or(Ok(()), |execution| execution.validate())
    }
}
