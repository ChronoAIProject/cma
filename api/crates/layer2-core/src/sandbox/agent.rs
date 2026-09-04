use serde::{Deserialize, Serialize};

use crate::ids::{AgentId, SandboxId, SubjectId, WorkspaceId, WorktreeId};
use crate::profiles::AgentProfileSnapshot;

use super::{AgentStatus, SandboxModelError, Worktree, WorktreeStatus};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FirstMessageDelivery {
    Pending,
    Delivering,
    Delivered,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversationState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    pub last_seq: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_message: Option<FirstMessageDelivery>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStopReason {
    Owner,
    Operator,
    RuntimeLost,
    SandboxRetired,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum AgentOrigin {
    Manual,
    Trigger {
        trigger_id: String,
        launch_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Agent {
    pub id: AgentId,
    pub worktree_id: WorktreeId,
    pub workspace_id: WorkspaceId,
    pub sandbox_id: SandboxId,
    pub owner: SubjectId,
    pub profile: AgentProfileSnapshot,
    pub status: AgentStatus,
    pub conversation: ConversationState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ready_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stopped_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<AgentStopReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted_at: Option<String>,
    pub origin: AgentOrigin,
    pub revision: u64,
}

impl Agent {
    pub fn validate_prerequisites(&self, worktree: &Worktree) -> Result<(), SandboxModelError> {
        if worktree.id != self.worktree_id
            || worktree.workspace_id != self.workspace_id
            || worktree.sandbox_id != self.sandbox_id
            || worktree.owner != self.owner
        {
            return Err(SandboxModelError::Prerequisite {
                resource: "agent",
                detail: "worktree lineage does not match",
            });
        }
        if worktree.status != WorktreeStatus::Ready {
            return Err(SandboxModelError::Prerequisite {
                resource: "agent",
                detail: "worktree is not ready",
            });
        }
        if worktree.agent_id.as_ref().is_some_and(|id| id != &self.id) {
            return Err(SandboxModelError::Prerequisite {
                resource: "agent",
                detail: "worktree is already assigned",
            });
        }
        if self.revision == 0 {
            return Err(SandboxModelError::invalid_value("agent.revision"));
        }
        Ok(())
    }

    pub fn transition(
        &mut self,
        next: AgentStatus,
        at: String,
        reason: Option<AgentStopReason>,
    ) -> Result<(), SandboxModelError> {
        if (next == AgentStatus::Stopped) != reason.is_some() {
            return Err(SandboxModelError::invalid_value("agent.stop_reason"));
        }
        let status = self.status.transition(next)?;
        let revision = self
            .revision
            .checked_add(1)
            .ok_or_else(|| SandboxModelError::invalid_value("agent.revision"))?;
        self.status = status;
        self.stop_reason = reason;
        self.updated_at = at.clone();
        match next {
            AgentStatus::Ready if self.ready_at.is_none() => self.ready_at = Some(at),
            AgentStatus::Stopped => self.stopped_at = Some(at),
            AgentStatus::Deleted => self.deleted_at = Some(at),
            _ => {}
        }
        self.revision = revision;
        Ok(())
    }
}
