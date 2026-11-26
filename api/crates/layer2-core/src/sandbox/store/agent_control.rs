use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::ids::{AgentId, SandboxId, SubjectId, WorkspaceId, WorktreeId};
use crate::sandbox::{Agent, AgentStatus, AgentStopReason, AgentWorkStep};

/// The exact aggregate snapshot an owner-authorized control request observed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentControlTarget {
    pub agent_id: AgentId,
    pub worktree_id: WorktreeId,
    pub workspace_id: WorkspaceId,
    pub sandbox_id: SandboxId,
    pub owner: SubjectId,
    pub expected_status: AgentStatus,
    pub expected_revision: u64,
}

impl From<&Agent> for AgentControlTarget {
    fn from(agent: &Agent) -> Self {
        Self {
            agent_id: agent.id.clone(),
            worktree_id: agent.worktree_id.clone(),
            workspace_id: agent.workspace_id.clone(),
            sandbox_id: agent.sandbox_id.clone(),
            owner: agent.owner.clone(),
            expected_status: agent.status,
            expected_revision: agent.revision,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentStopRequest {
    pub target: AgentControlTarget,
    pub reason: AgentStopReason,
    pub requested_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentDeleteRequest {
    pub target: AgentControlTarget,
    pub requested_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentControlResult {
    /// This request performed the single durable scheduling mutation.
    Scheduled(Agent),
    /// The same durable workflow was already in progress or complete.
    Existing(Agent),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AgentControlStoreError {
    #[error("agent not found")]
    NotFound,
    #[error("agent status does not permit the requested control operation: {status:?}")]
    StatusConflict { status: AgentStatus },
    #[error("agent work conflicts with the requested control operation: {step:?}")]
    WorkConflict { step: AgentWorkStep },
    #[error("agent stop reason conflicts with the durable request")]
    StopReasonConflict,
    #[error("agent changed while the control operation was requested")]
    Conflict,
    #[error("an exact native Agent Close observation is required")]
    RestartEvidenceRequired,
    #[error("the stopped Agent's retained turn could not be settled")]
    RestartTurnUnsettled,
    #[error("agent worktree recovery is not eligible")]
    WorktreeRecoveryConflict,
    #[error("agent limit reached: {live} of {limit}")]
    LimitReached { limit: u8, live: u8 },
    #[error("stored agent data is not usable: {0}")]
    Corrupt(String),
    #[error("agent control store unavailable: {0}")]
    Unavailable(String),
}

impl AgentControlResult {
    pub fn agent(&self) -> &Agent {
        match self {
            Self::Scheduled(agent) | Self::Existing(agent) => agent,
        }
    }

    pub fn into_agent(self) -> Agent {
        match self {
            Self::Scheduled(agent) | Self::Existing(agent) => agent,
        }
    }
}

/// Atomic owner authorization, aggregate CAS, lineage validation, and work scheduling.
#[async_trait]
pub trait AgentControlStore: Send + Sync {
    /// Recover only a never-ready failed checkout. Stores must atomically fence all
    /// parents, reserve a new worktree, and retain the original failed worktree.
    async fn begin_agent_worktree_recovery(
        &self,
        _id: &AgentId,
        _caller: &crate::Caller,
        _operation_id: &str,
        _successor_id: &WorktreeId,
        _now: String,
    ) -> Result<Agent, AgentControlStoreError> {
        Err(AgentControlStoreError::WorktreeRecoveryConflict)
    }

    /// Read an exact acceptance or atomically reserve capacity for a proven restart.
    /// A replay never needs a fresh runtime observation.
    async fn schedule_agent_restart(
        &self,
        _request: super::AgentRestartRequest,
    ) -> Result<super::AgentRestartAcceptance, AgentControlStoreError> {
        Err(AgentControlStoreError::Conflict)
    }

    /// Schedule stop once; matching active stop work is an idempotent replay.
    async fn schedule_agent_stop(
        &self,
        request: AgentStopRequest,
    ) -> Result<AgentControlResult, AgentControlStoreError>;

    /// Schedule delete once; deletion work and `Deleted` are idempotent replays.
    async fn schedule_agent_delete(
        &self,
        request: AgentDeleteRequest,
    ) -> Result<AgentControlResult, AgentControlStoreError>;
}
