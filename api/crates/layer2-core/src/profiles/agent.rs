use serde::{Deserialize, Serialize};

use crate::ids::{AgentProfileId, SkillId, SubjectId};

use super::common::{validate_metadata, AgentProfileRef, ProfileError, ProfileVisibility};
use super::content_digest;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThinkingEffort {
    Minimal,
    Low,
    Medium,
    High,
    XHigh,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionLevel {
    Guarded,
    Autonomous,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillSource {
    Ornn,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillRef {
    pub source: SkillSource,
    pub id: SkillId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentProfileContent {
    /// Isolate this agent's filesystem, processes and local network from siblings.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub isolated_agent: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub thinking_effort: ThinkingEffort,
    pub permission_level: PermissionLevel,
    #[serde(default)]
    pub skills: Vec<SkillRef>,
    #[serde(default)]
    pub system_prompt: String,
}

impl AgentProfileContent {
    pub fn validate(&self) -> Result<(), ProfileError> {
        if self
            .model
            .as_ref()
            .is_some_and(|model| model.trim().is_empty())
        {
            return Err(ProfileError::new("invalid_model", "model"));
        }
        if self.skills.len() > 32 {
            return Err(ProfileError::new("too_many_skills", "skills"));
        }
        if self.system_prompt.len() > 32 * 1_024 {
            return Err(ProfileError::new("system_prompt_too_long", "system_prompt"));
        }
        if self.skills.iter().any(|skill| skill.id.as_str().is_empty()) {
            return Err(ProfileError::new("invalid_skill", "skills"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentProfile {
    pub id: AgentProfileId,
    pub owner: SubjectId,
    pub name: String,
    pub description: String,
    pub visibility: ProfileVisibility,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forked_from: Option<AgentProfileRef>,
    #[serde(default)]
    pub uses_count: u64,
    pub draft: AgentProfileContent,
    pub draft_version: u64,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retired_at: Option<String>,
}

impl AgentProfile {
    pub fn validate(&self) -> Result<(), ProfileError> {
        validate_metadata(&self.name, &self.description)?;
        self.draft.validate()
    }

    pub fn publish(
        &self,
        revision: u32,
        published_at: String,
    ) -> Result<AgentProfileRevision, ProfileError> {
        self.validate()?;
        Ok(AgentProfileRevision {
            profile_id: self.id.clone(),
            revision,
            digest: content_digest(&self.draft)?,
            content: self.draft.clone(),
            published_at,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentProfileRevision {
    pub profile_id: AgentProfileId,
    pub revision: u32,
    pub content: AgentProfileContent,
    pub digest: String,
    pub published_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentProfileSnapshot {
    pub id: AgentProfileId,
    pub revision: u32,
    pub digest: String,
    pub content: AgentProfileContent,
}

impl From<&AgentProfileRevision> for AgentProfileSnapshot {
    fn from(revision: &AgentProfileRevision) -> Self {
        Self {
            id: revision.profile_id.clone(),
            revision: revision.revision,
            digest: revision.digest.clone(),
            content: revision.content.clone(),
        }
    }
}
