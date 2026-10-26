//! Additive Agent skills and durable runtime-confirmed installation state.

use super::Agents;
use crate::{Error, IdempotencyKey, Result, profiles::SkillSource};
use reqwest::{Method, StatusCode};
use serde::{Deserialize, Serialize};

string_enum! { /// Durable state of an Agent skill installation.
    pub enum AgentSkillStatus { Pending => "pending", Installed => "installed", Failed => "failed" }
}

/// One profile or subsequently installed Agent skill.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AgentSkill {
    /// Registry-local skill identifier.
    pub id: String,
    /// Display name when the registry material has resolved one.
    pub name: Option<String>,
    /// Exact registry version.
    pub version: String,
    /// Registry supplying this skill.
    pub source: SkillSource,
    /// Durable installation state, including runtime confirmation.
    pub status: AgentSkillStatus,
    /// Public failure detail for a failed installation, when supplied.
    pub error: Option<String>,
}

/// Skill inventory and current installation capability for the owned Agent.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AgentSkills {
    /// Profile and additive skills known to this Agent.
    pub items: Vec<AgentSkill>,
    /// Whether an installation is still awaiting runtime confirmation.
    pub pending: bool,
    /// Whether a new skill can currently be installed.
    pub can_install: bool,
    /// User-facing explanation when installation is unavailable.
    pub unavailable_reason: Option<String>,
}

/// Install one explicitly selected Ornn skill at an exact version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstallAgentSkillRequest {
    /// At most 64 ASCII letters, digits, hyphens, underscores, or dots;
    /// a leading dot and built-in skill identities are not accepted.
    pub id: String,
    /// Exact version of at most 256 bytes, without surrounding whitespace,
    /// control characters, or a leading `@`.
    pub version: String,
}

impl Agents<'_> {
    /// Read skills and whether this runtime currently permits installation.
    pub async fn skills(&self, id: &str) -> Result<AgentSkills> {
        self.client.get(&["agents", id, "skills"], &[]).await
    }

    /// Admit an additive skill installation for an idle Agent. A `202` receipt
    /// is not runtime confirmation; read `skills()` until `pending` is false.
    /// Retry uncertain replies with the same body and idempotency key.
    pub async fn install_skill(
        &self,
        id: &str,
        body: &InstallAgentSkillRequest,
        key: &IdempotencyKey,
    ) -> Result<AgentSkills> {
        if body.id.is_empty()
            || body.id.len() > 64
            || body.id.starts_with('.')
            || !body
                .id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
            || matches!(body.id.as_str(), "nyxid" | "ornn-agent-manual-cli")
        {
            return Err(Error::validation("invalid Agent skill identity"));
        }
        if body.version.is_empty()
            || body.version.len() > 256
            || body.version.starts_with('@')
            || body.version.trim() != body.version
            || body.version.chars().any(char::is_control)
        {
            return Err(Error::validation("invalid Agent skill version"));
        }
        self.client
            .command(
                Method::POST,
                &["agents", id, "skills"],
                Some(body),
                key,
                StatusCode::ACCEPTED,
            )
            .await
    }
}
