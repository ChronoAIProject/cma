//! Content-pinned skill installation and replacement in an existing Agent home.
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upgrade: Option<Box<AgentSkillUpgrade>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentSkillUpgrade {
    pub sequence: u64,
    pub previous: AgentSkill,
    /// Absent only for a skill materialized with the immutable Agent home.
    pub previous_receipt: Option<AgentSkillInstallReceipt>,
}

impl AgentSkillInstallRequest {
    pub fn new(agent_id: AgentSlot, skill: AgentSkill) -> Self {
        let operation_id = format!("agent:{}:skill:{}:install:v1", agent_id.as_str(), skill.id);
        let mut value = Self {
            operation_id,
            request_sha256: String::new(),
            agent_id,
            skill,
            upgrade: None,
        };
        value.request_sha256 = value.computed_sha256();
        value
    }
    pub fn upgrade(
        agent_id: AgentSlot,
        skill: AgentSkill,
        previous: AgentSkill,
        previous_receipt: Option<AgentSkillInstallReceipt>,
        sequence: u64,
    ) -> Self {
        let mut value = Self::new(agent_id, skill);
        value.upgrade = Some(Box::new(AgentSkillUpgrade {
            sequence,
            previous,
            previous_receipt,
        }));
        value.operation_id = value.expected_operation_id();
        value.request_sha256 = value.computed_sha256();
        value
    }
    pub fn sequence(&self) -> u64 {
        self.upgrade.as_ref().map_or(0, |upgrade| upgrade.sequence)
    }
    pub fn expected_operation_id(&self) -> String {
        match &self.upgrade {
            Some(upgrade) => format!(
                "agent:{}:skill:{}:upgrade:{}:v1",
                self.agent_id.as_str(),
                self.skill.id,
                upgrade.sequence
            ),
            None => format!(
                "agent:{}:skill:{}:install:v1",
                self.agent_id.as_str(),
                self.skill.id
            ),
        }
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
        if let Some(upgrade) = &self.upgrade {
            digest.update(b"\0upgrade:v1\0");
            digest.update(
                serde_json::to_vec(&(
                    upgrade.sequence,
                    &upgrade.previous.id,
                    &upgrade.previous.name,
                    &upgrade.previous.version,
                    &upgrade.previous.archive_sha256,
                    &upgrade.previous_receipt,
                ))
                .expect("serializable previous installation"),
            );
        }
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

#[cfg(test)]
#[path = "agent_skill_install_tests.rs"]
mod tests;

/// Each preparation budget is durable and belongs to one immutable upgrade request.
pub const MAX_AGENT_SKILL_UPGRADE_PREPARATION_ATTEMPTS: u32 = 5;

/// Terminal, owner-actionable replacement failures; no runtime loss is implied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AgentSkillUpgradeRefusal {
    #[error("Save edits outside the skill directory and restore the installed archive's files before retrying.")]
    Modified,
    #[error("Restore access to the exact previous archive or its checksum-verified warm cache before retrying.")]
    PreviousUnavailable,
    #[error("Archive and remove settled trees in the Agent home's .cma-skill-upgrades directory; preserve pending evidence, then retry.")]
    RetentionFull,
    #[error("The previous version remains effective. Preserve the staged trees; save and restore local skill edits, then retry with a new key.")]
    RecoveryConflict,
    #[error("The previous version remains effective. Preparation failed five times; restore the archive or runtime dependency, then retry with a new key.")]
    PreparationExhausted,
}
impl AgentSkillUpgradeRefusal {
    pub const fn code(self) -> &'static str {
        match self {
            Self::Modified => "agent_skill_upgrade_modified",
            Self::PreviousUnavailable => "agent_skill_upgrade_previous_unavailable",
            Self::RetentionFull => "agent_skill_upgrade_retention_full",
            Self::RecoveryConflict => "agent_skill_upgrade_recovery_conflict",
            Self::PreparationExhausted => "agent_skill_upgrade_preparation_exhausted",
        }
    }
}
