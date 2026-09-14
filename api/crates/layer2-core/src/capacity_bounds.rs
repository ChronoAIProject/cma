//! Validated resource-capacity requests and deployment bounds.

use serde::{Deserialize, Serialize};

/// CPU and memory requested from a runtime provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestedCapacity {
    pub cpu_millis: u32,
    pub memory_mib: u32,
}

impl RequestedCapacity {
    pub fn new(cpu_millis: u32, memory_mib: u32) -> Result<Self, crate::CoreError> {
        if cpu_millis == 0 || memory_mib == 0 {
            return Err(crate::CoreError::InvalidRequest(
                "resource capacity must have non-zero CPU and memory".into(),
            ));
        }
        Ok(Self {
            cpu_millis,
            memory_mib,
        })
    }
}

/// Inclusive deployment bounds for one resource-capacity request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapacityBounds {
    pub min_cpu_millis: u32,
    pub max_cpu_millis: u32,
    pub min_memory_mib: u32,
    pub max_memory_mib: u32,
}

impl CapacityBounds {
    pub fn new(
        min_cpu_millis: u32,
        max_cpu_millis: u32,
        min_memory_mib: u32,
        max_memory_mib: u32,
    ) -> Result<Self, crate::CoreError> {
        if min_cpu_millis == 0
            || min_memory_mib == 0
            || min_cpu_millis > max_cpu_millis
            || min_memory_mib > max_memory_mib
        {
            return Err(crate::CoreError::InvalidRequest(
                "capacity bounds require ordered, non-zero CPU and memory limits".into(),
            ));
        }
        Ok(Self {
            min_cpu_millis,
            max_cpu_millis,
            min_memory_mib,
            max_memory_mib,
        })
    }

    pub fn check(self, request: RequestedCapacity) -> Result<(), OutOfBounds> {
        for (dimension, requested, minimum, maximum) in [
            (
                CapacityDimension::CpuMillis,
                request.cpu_millis,
                self.min_cpu_millis,
                self.max_cpu_millis,
            ),
            (
                CapacityDimension::MemoryMib,
                request.memory_mib,
                self.min_memory_mib,
                self.max_memory_mib,
            ),
        ] {
            if !(minimum..=maximum).contains(&requested) {
                return Err(OutOfBounds {
                    dimension,
                    requested,
                    minimum,
                    maximum,
                });
            }
        }
        Ok(())
    }
}

/// Resource dimension that violated a deployment bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapacityDimension {
    CpuMillis,
    MemoryMib,
}

/// A request lies outside one inclusive resource bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("{dimension:?} request {requested} is outside {minimum}..={maximum}")]
pub struct OutOfBounds {
    pub dimension: CapacityDimension,
    pub requested: u32,
    pub minimum: u32,
    pub maximum: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks_each_capacity_dimension() {
        let bounds = CapacityBounds::new(250, 4_000, 512, 8_192).unwrap();
        assert!(bounds
            .check(RequestedCapacity::new(1_000, 2_048).unwrap())
            .is_ok());
        assert_eq!(
            bounds
                .check(RequestedCapacity::new(125, 2_048).unwrap())
                .unwrap_err()
                .dimension,
            CapacityDimension::CpuMillis
        );
        assert!(CapacityBounds::new(0, 4_000, 512, 8_192).is_err());
        assert!(RequestedCapacity::new(1_000, 0).is_err());
    }
}
