use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Stable slot identity. It is always the owning agent's identifier.
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AgentSlot(pub String);

impl AgentSlot {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Model reasoning setting supported by the agent profile.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningEffort {
    Minimal,
    Low,
    #[default]
    Medium,
    High,
    #[serde(rename = "xhigh")]
    XHigh,
}

impl ReasoningEffort {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::XHigh => "xhigh",
        }
    }
}

/// Alias matching the profile-layer name without coupling the two layers.
pub type ThinkingEffort = ReasoningEffort;

/// Authority granted to an agent inside the outer sandbox.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionLevel {
    #[default]
    Guarded,
    Autonomous,
}

/// Runtime choices carried on every agent open.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub reasoning_effort: ReasoningEffort,
    pub permission: PermissionLevel,
    pub own_credential: bool,
    pub working_directory: String,
    pub home_directory: String,
    pub slot: AgentSlot,
}

/// One requested skill archive fetched by the runtime supervisor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentSkill {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub archive_sha256: String,
    pub source_url: String,
}

/// Agent-profile material to render into a provider-specific home.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentHomeRequest {
    pub options: RuntimeOptions,
    #[serde(default)]
    pub system_prompt: String,
    #[serde(default)]
    pub skills: Vec<AgentSkill>,
}

/// Open one agent after its home and worktree exist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentOpenRequest {
    pub options: RuntimeOptions,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_conversation: Option<String>,
}

/// One relative file materialized beneath an agent home.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HomeFile {
    pub path: String,
    pub bytes: Vec<u8>,
}

/// One relative link materialized beneath an agent home.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HomeSymlink {
    pub path: String,
    pub target: String,
}

/// Provider-specific home plan transported without interpreting its contents.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentHomePlan {
    pub agent_id: AgentSlot,
    #[serde(default)]
    pub files: Vec<HomeFile>,
    #[serde(default)]
    pub symlinks: Vec<HomeSymlink>,
    #[serde(default)]
    pub skills: Vec<AgentSkill>,
}

/// Exact process launch for one agent slot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentLaunch {
    pub agent_id: AgentSlot,
    pub cwd: String,
    pub home: String,
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

/// Opaque captured conversation archive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversationSnapshot {
    pub conversation_id: String,
    pub archive_sha256: String,
    pub archive: Vec<u8>,
}
