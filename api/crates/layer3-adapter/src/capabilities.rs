use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{AdapterError, RuntimeContractMismatch};

pub const RUNTIME_CONTRACT_LABEL: &str = "dev.chrono.cma.runtime-contract";
pub const RUNTIME_CONTRACT_VERSION: &str = "3";

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
}

/// An admitted runtime contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeContract {
    V3,
}

impl RuntimeContract {
    /// Admit only the exact label supported by this control plane.
    pub fn admit(image: &str, labels: &BTreeMap<String, String>) -> Result<Self, AdapterError> {
        let found = labels.get(RUNTIME_CONTRACT_LABEL);
        if found.map(String::as_str) == Some(RUNTIME_CONTRACT_VERSION) {
            Ok(Self::V3)
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
            },
        }
    }
}
