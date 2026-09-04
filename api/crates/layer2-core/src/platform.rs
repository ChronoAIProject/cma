//! Deployment-owned policy that is independent of a resource aggregate.

use serde::{Deserialize, Serialize};

use crate::{
    CapacityBounds, CountLimit, CpuPressurePolicy, DeploymentCapacity, MemoryPressurePolicy,
};

/// Retained platform settings while resource-specific settings move to #814.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlatformSettings {
    pub capacity_bounds: Option<CapacityBounds>,
    pub deployment_capacity: Option<DeploymentCapacity>,
    pub resource_limit: CountLimit,
    pub cpu_pressure: CpuPressurePolicy,
    pub memory_pressure: MemoryPressurePolicy,
}

impl PlatformSettings {
    pub fn validate(&self) -> Result<(), crate::CoreError> {
        CountLimit::new(self.resource_limit.maximum())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid_and_provider_capacity_is_optional() {
        let settings = PlatformSettings::default();
        assert!(settings.validate().is_ok());
        assert!(settings.capacity_bounds.is_none());
        assert!(settings.deployment_capacity.is_none());
    }
}
