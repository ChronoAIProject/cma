//! Mutable scheduling policy, independent of an instance's pinned environment.

use serde::{Deserialize, Serialize};

use super::{ProfileError, ProfileVisibility};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxProfileAvailability {
    pub warmup_enabled: bool,
    pub warmup_count: u8,
    pub always_keep_alive: bool,
}

impl Default for SandboxProfileAvailability {
    fn default() -> Self {
        Self {
            warmup_enabled: false,
            warmup_count: 1,
            always_keep_alive: false,
        }
    }
}

impl SandboxProfileAvailability {
    pub fn validate(&self, visibility: ProfileVisibility) -> Result<(), ProfileError> {
        if !(1..=5).contains(&self.warmup_count) {
            return Err(ProfileError::new(
                "invalid_warmup_count",
                "availability.warmup_count",
            ));
        }
        if self.warmup_enabled && visibility != ProfileVisibility::Public {
            return Err(ProfileError::new(
                "warmup_requires_public_profile",
                "availability.warmup_enabled",
            ));
        }
        Ok(())
    }
}
