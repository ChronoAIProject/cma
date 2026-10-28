use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{AdapterError, RuntimeContractMismatch};

pub const RUNTIME_CONTRACT_LABEL: &str = "dev.chrono.cma.runtime-contract";
pub const RUNTIME_CONTRACT_VERSION: &str = "3";
/// Explicit image evidence for a Linux desktop session.
pub const RUNTIME_DESKTOP_LABEL: &str = "dev.chrono.cma.runtime-desktop";

/// Static abilities proved by a runtime contract.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdapterCapabilities {
    pub conversation: bool,
    pub snapshot: bool,
    pub per_agent_home: bool,
    pub steer: bool,
    pub interrupt: bool,
    pub usage: bool,
    pub resources: bool,
    #[serde(default)]
    pub desktop: bool,
}

/// An admitted runtime contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeContract {
    V3,
    V3Desktop,
}

impl RuntimeContract {
    /// Admit only the exact label supported by this control plane.
    pub fn admit(image: &str, labels: &BTreeMap<String, String>) -> Result<Self, AdapterError> {
        let found = labels.get(RUNTIME_CONTRACT_LABEL);
        if found.map(String::as_str) == Some(RUNTIME_CONTRACT_VERSION) {
            if labels.get(RUNTIME_DESKTOP_LABEL).map(String::as_str) == Some("1") {
                Ok(Self::V3Desktop)
            } else {
                Ok(Self::V3)
            }
        } else {
            Err(RuntimeContractMismatch {
                image: image.to_string(),
                expected: RUNTIME_CONTRACT_VERSION.to_string(),
                found: found.cloned(),
            }
            .into())
        }
    }

    /// Capabilities are a property of the versioned contract, not an image name.
    pub const fn capabilities(self) -> AdapterCapabilities {
        match self {
            Self::V3 => AdapterCapabilities {
                conversation: true,
                snapshot: true,
                per_agent_home: true,
                steer: true,
                interrupt: true,
                usage: true,
                resources: true,
                desktop: false,
            },
            Self::V3Desktop => AdapterCapabilities {
                conversation: true,
                snapshot: true,
                per_agent_home: true,
                steer: true,
                interrupt: true,
                usage: true,
                resources: true,
                desktop: true,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_is_admitted_only_from_an_explicit_image_label() {
        let labels = BTreeMap::from([(
            RUNTIME_CONTRACT_LABEL.to_string(),
            RUNTIME_CONTRACT_VERSION.to_string(),
        )]);
        assert_eq!(
            RuntimeContract::admit("runtime", &labels).unwrap(),
            RuntimeContract::V3
        );
        assert!(!RuntimeContract::V3.capabilities().desktop);

        let desktop = BTreeMap::from([
            (
                RUNTIME_CONTRACT_LABEL.to_string(),
                RUNTIME_CONTRACT_VERSION.to_string(),
            ),
            (RUNTIME_DESKTOP_LABEL.to_string(), "1".into()),
        ]);
        assert_eq!(
            RuntimeContract::admit("runtime-desktop", &desktop).unwrap(),
            RuntimeContract::V3Desktop
        );
        assert!(RuntimeContract::V3Desktop.capabilities().desktop);
    }
}
