//! One additive, content-pinned skill installation into an existing Agent home.
use crate::{AgentSkill, AgentSlot};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentSkillInstallRequest {
    pub operation_id: String,
    pub request_sha256: String,
    pub agent_id: AgentSlot,
    pub skill: AgentSkill,
}

impl AgentSkillInstallRequest {
    pub fn new(agent_id: AgentSlot, skill: AgentSkill) -> Self {
        let operation_id = format!("agent:{}:skill:{}:install:v1", agent_id.as_str(), skill.id);
        let mut value = Self {
            operation_id,
            request_sha256: String::new(),
            agent_id,
            skill,
        };
        value.request_sha256 = value.computed_sha256();
        value
    }
    pub fn computed_sha256(&self) -> String {
        // The replaceable archive URL carries no content identity or credentials.
        let content = serde_json::to_vec(&(
            &self.agent_id,
            &self.skill.id,
            &self.skill.name,
            &self.skill.version,
            &self.skill.archive_sha256,
        ))
        .expect("serializable skill material");
        let mut digest = Sha256::new();
        digest.update(b"chrono.agent.skill.install.v1\0");
        digest.update(content);
        format!("{:x}", digest.finalize())
    }
    pub fn has_valid_sha256(&self) -> bool {
        self.request_sha256 == self.computed_sha256()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentSkillInstallReceipt {
    pub operation_id: String,
    pub request_sha256: String,
    pub agent_id: AgentSlot,
    pub skill_id: String,
    pub archive_sha256: String,
}
impl AgentSkillInstallReceipt {
    pub fn from_request(request: &AgentSkillInstallRequest) -> Self {
        Self {
            operation_id: request.operation_id.clone(),
            request_sha256: request.request_sha256.clone(),
            agent_id: request.agent_id.clone(),
            skill_id: request.skill.id.clone(),
            archive_sha256: request.skill.archive_sha256.clone(),
        }
    }
    pub fn matches(&self, request: &AgentSkillInstallRequest) -> bool {
        *self == Self::from_request(request)
    }
}
