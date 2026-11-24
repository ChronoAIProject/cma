//! Public contracts for owner-scoped Agent reads.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::ids::{AgentId, SandboxId, WorkspaceId};

use super::{Agent, AgentStatus, Sandbox, TypedFailure, Workspace, Worktree};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentListQuery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox_id: Option<SandboxId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<WorkspaceId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<AgentStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum AgentTurnOutcome {
    Active,
    Completed { at: String },
    Failed { at: String, failure: TypedFailure },
    Interrupted { at: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentTranscriptMessage {
    pub sequence: u64,
    pub role: AgentTranscriptRole,
    pub content: String,
    pub appended_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentTranscriptRole {
    Owner,
    Agent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentTranscriptTurn {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retained_output: Option<super::AgentRetainedOutputAvailability>,
    pub turn_sequence: u64,
    pub started_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_key: Option<String>,
    pub owner_message: String,
    pub agent_messages: Vec<String>,
    pub messages: Vec<AgentTranscriptMessage>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub metadata: std::collections::BTreeMap<String, String>,
    pub outcome: AgentTurnOutcome,
}

impl AgentTranscriptTurn {
    pub fn final_answer(&self) -> Option<&AgentTranscriptMessage> {
        if !matches!(self.outcome, AgentTurnOutcome::Completed { .. }) {
            return None;
        }
        self.messages
            .iter()
            .max_by_key(|message| message.sequence)
            .filter(|message| {
                message.role == AgentTranscriptRole::Agent && !message.content.trim().is_empty()
            })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentTranscript {
    pub agent_id: AgentId,
    pub through_seq: u64,
    pub turns: Vec<AgentTranscriptTurn>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentUsage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_turn: Option<super::AgentTurnUsage>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub context_used: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentUsageReading {
    pub agent_id: AgentId,
    pub through_seq: u64,
    pub usage: AgentUsage,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxResourceReading {
    pub uptime_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_usage_usec: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_millis_limit: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_bytes_used: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_bytes_limit: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_bytes_peak: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stall_permille: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ceiling_hits: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allocation_failures: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kills: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub received_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sent_bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentResourceReading {
    pub agent_id: AgentId,
    pub sandbox_id: SandboxId,
    pub reading: SandboxResourceReading,
}

/// Exact owner-validated hierarchy for one Agent read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentLineage {
    pub agent: Agent,
    pub worktree: Worktree,
    pub workspace: Workspace,
    pub sandbox: Sandbox,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SandboxResourceReadError {
    #[error(transparent)]
    RuntimeCapacity(super::RuntimeCapacityError),
    #[error("sandbox does not support resource readings: {0}")]
    Unsupported(String),
    #[error("sandbox resource binding is corrupt: {0}")]
    Corrupt(String),
    #[error("sandbox resource reader is unavailable: {0}")]
    Unavailable(String),
}

#[async_trait]
pub trait SandboxResourceReader: Send + Sync {
    async fn sandbox_resources(
        &self,
        sandbox: &Sandbox,
    ) -> Result<SandboxResourceReading, SandboxResourceReadError>;
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AgentReadError {
    #[error(transparent)]
    RuntimeCapacity(super::RuntimeCapacityError),
    #[error(transparent)]
    Access(crate::resource_access::ResourceAccessError),
    #[error("forbidden")]
    Forbidden,
    #[error("agent not found")]
    NotFound,
    #[error("invalid page request")]
    InvalidPage,
    #[error("transcript exceeds the complete result budget; request limit=32 and follow next")]
    TranscriptPaginationRequired,
    #[error("agent was not granted runtime capability: {capability:?}")]
    CapabilityUnsupported {
        capability: crate::RuntimeCapability,
    },
    #[error("agent sandbox does not support resource readings: {0}")]
    ResourcesUnsupported(String),
    #[error("history runtime is unavailable")]
    HistoryRuntimeUnavailable,
    #[error("agent data is not usable: {0}")]
    Corrupt(String),
    #[error("agent read dependency is unavailable: {0}")]
    Unavailable(String),
}

impl AgentReadError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::RuntimeCapacity(capacity) => capacity.code(),
            Self::Access(_) => "resource_access_failed",
            Self::Forbidden => "forbidden",
            Self::NotFound => "agent_not_found",
            Self::InvalidPage => "invalid_page",
            Self::TranscriptPaginationRequired => "transcript_pagination_required",
            Self::CapabilityUnsupported { .. } => "agent_capability_unsupported",
            Self::ResourcesUnsupported(_) => "agent_resources_unsupported",
            Self::HistoryRuntimeUnavailable => "history_runtime_unavailable",
            Self::Corrupt(_) => "stored_agent_corrupt",
            Self::Unavailable(_) => "agent_read_unavailable",
        }
    }
}
