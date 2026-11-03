use serde::Serialize;
use sha2::{Digest, Sha256};

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EnvironmentAvailability {
    Available,
    Unsupported,
    NotEnabled,
    Forbidden,
    TemporarilyUnavailable,
}

#[derive(Debug, Serialize)]
pub struct EnvironmentFeature {
    pub state: EnvironmentAvailability,
    pub reason: &'static str,
    pub scope: &'static str,
    pub writable: bool,
}

#[derive(Debug, Serialize)]
pub struct AgentEnvironment {
    pub agent_id: String,
    pub policy_revision: u64,
    pub runtime_incarnation: Option<String>,
    pub revalidate_at: Option<String>,
    pub files: EnvironmentFeature,
    pub operation_output: EnvironmentFeature,
    pub browser: EnvironmentFeature,
    pub replay: EnvironmentFeature,
    pub control: EnvironmentFeature,
}

impl AgentEnvironment {
    pub(super) fn incarnation(
        agent_id: &crate::AgentId,
        sandbox: &super::Sandbox,
    ) -> Option<String> {
        let runtime = sandbox.runtime.as_ref()?;
        let boot = runtime.boot_id.as_ref()?;
        let generation = runtime.provision_generation?;
        let mut digest = Sha256::new();
        digest.update(b"cma-agent-environment-v1\0");
        digest.update(agent_id.as_str());
        digest.update([0]);
        digest.update(generation.to_be_bytes());
        digest.update(boot);
        Some(format!("{:x}", digest.finalize()))
    }
}

impl EnvironmentFeature {
    pub(super) fn history(allowed: bool, reason: &'static str) -> Self {
        Self {
            state: if allowed {
                EnvironmentAvailability::Available
            } else {
                EnvironmentAvailability::Forbidden
            },
            reason: if allowed {
                reason
            } else {
                "view_history_required"
            },
            scope: "agent_turn_history",
            writable: false,
        }
    }

    pub(super) fn unsupported() -> Self {
        Self {
            state: EnvironmentAvailability::Unsupported,
            reason: "not_supported_by_contract",
            scope: "none",
            writable: false,
        }
    }
}
