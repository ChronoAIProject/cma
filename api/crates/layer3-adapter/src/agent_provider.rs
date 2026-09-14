use serde::{Deserialize, Serialize};

/// Closed selection of agent implementations registered by this adapter layer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentProvider {
    #[default]
    Codex,
}

impl AgentProvider {
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
