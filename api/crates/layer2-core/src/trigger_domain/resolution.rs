use serde::{Deserialize, Serialize};

use crate::{AgentId, SandboxId, WorkspaceId};

use super::TriggerError;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", content = "id", rename_all = "snake_case")]
pub enum ResolvedResource<T> {
    Pending,
    Reused(T),
    Created(T),
}

impl<T> ResolvedResource<T> {
    pub const fn is_pending(&self) -> bool {
        matches!(self, Self::Pending)
    }

    pub fn id(&self) -> Option<&T> {
        match self {
            Self::Pending => None,
            Self::Reused(id) | Self::Created(id) => Some(id),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchResolution {
    pub sandbox: ResolvedResource<SandboxId>,
    pub workspace: ResolvedResource<WorkspaceId>,
    pub agent: ResolvedResource<AgentId>,
    pub choice_required: bool,
}

impl LaunchResolution {
    pub const fn pending() -> Self {
        Self {
            sandbox: ResolvedResource::Pending,
            workspace: ResolvedResource::Pending,
            agent: ResolvedResource::Pending,
            choice_required: false,
        }
    }

    pub fn validate(&self) -> Result<(), TriggerError> {
        if self.sandbox.is_pending() && (!self.workspace.is_pending() || !self.agent.is_pending()) {
            return Err(TriggerError::new(
                "invalid_launch_resolution",
                "resolution.sandbox",
            ));
        }
        if self.workspace.is_pending() && !self.agent.is_pending() {
            return Err(TriggerError::new(
                "invalid_launch_resolution",
                "resolution.workspace",
            ));
        }
        if self.choice_required && !self.agent.is_pending() {
            return Err(TriggerError::new(
                "invalid_launch_resolution",
                "resolution.choice_required",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepeatChoice {
    Resume,
    New,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExistingAgentChoice {
    pub agent_id: AgentId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub created_at: String,
    pub first_message_excerpt: String,
}
