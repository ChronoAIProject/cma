//! Arithmetic for deployment-wide runtime-provider capacity.

use serde::{Deserialize, Serialize};

use crate::RequestedCapacity;

/// Total resources the deployment may reserve from its provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeploymentCapacity {
    pub cpu_millis: u64,
    pub memory_mib: u64,
}

impl DeploymentCapacity {
    pub fn admits(self, reserved: ReservedCapacity, request: RequestedCapacity) -> bool {
        reserved
            .cpu_millis
            .checked_add(u64::from(request.cpu_millis))
            .is_some_and(|cpu| cpu <= self.cpu_millis)
            && reserved
                .memory_mib
                .checked_add(u64::from(request.memory_mib))
                .is_some_and(|memory| memory <= self.memory_mib)
    }

    pub fn available_after(self, reserved: ReservedCapacity) -> AvailableCapacity {
        AvailableCapacity {
            cpu_millis: self.cpu_millis.saturating_sub(reserved.cpu_millis),
            memory_mib: self.memory_mib.saturating_sub(reserved.memory_mib),
        }
    }
}

/// Resources already reserved by live provider allocations.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReservedCapacity {
    pub resources: u64,
    pub cpu_millis: u64,
    pub memory_mib: u64,
}

impl ReservedCapacity {
    pub fn include(&mut self, capacity: RequestedCapacity) {
        self.resources = self.resources.saturating_add(1);
        self.cpu_millis = self
            .cpu_millis
            .saturating_add(u64::from(capacity.cpu_millis));
        self.memory_mib = self
            .memory_mib
            .saturating_add(u64::from(capacity.memory_mib));
    }
}

/// Deployment resources not consumed by known reservations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AvailableCapacity {
    pub cpu_millis: u64,
    pub memory_mib: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admission_uses_checked_two_dimensional_arithmetic() {
        let total = DeploymentCapacity {
            cpu_millis: 4_000,
            memory_mib: 8_192,
        };
        let mut reserved = ReservedCapacity::default();
        reserved.include(RequestedCapacity::new(3_000, 2_048).unwrap());
        assert!(total.admits(reserved, RequestedCapacity::new(1_000, 6_144).unwrap()));
        assert!(!total.admits(reserved, RequestedCapacity::new(1_001, 1_024).unwrap()));
        assert_eq!(
            total.available_after(reserved),
            AvailableCapacity {
                cpu_millis: 1_000,
                memory_mib: 6_144,
            }
        );
    }
}
