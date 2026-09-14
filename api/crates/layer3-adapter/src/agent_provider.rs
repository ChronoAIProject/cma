use serde::{Deserialize, Serialize};

/// Closed selection of agent implementations registered by this adapter layer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentProvider {
    #[default]
    Codex,
}

/// Provider-owned connection identifiers consumed by the neutral profile policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelConnectionServices {
    pub managed: &'static str,
    pub legacy_managed: &'static str,
    /// An internal connection requirement, never an AgentKey service grant.
    pub subscription: &'static str,
}

impl ModelConnectionServices {
    pub fn contains(self, service: &str) -> bool {
        [self.managed, self.legacy_managed, self.subscription].contains(&service)
    }
}

impl AgentProvider {
    pub fn connection_services(self) -> ModelConnectionServices {
        match self {
            Self::Codex => crate::codex::CONNECTION_SERVICES,
        }
    }

    /// Provider-owned environment keys that a Sandbox profile may not override.
    pub fn reserved_environment_names(self) -> &'static [&'static str] {
        match self {
            Self::Codex => crate::codex::RESERVED_ENVIRONMENT_NAMES,
        }
    }
}

#[cfg(test)]
#[path = "agent_provider_tests.rs"]
mod tests;
