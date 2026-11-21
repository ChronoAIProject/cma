use serde::{Deserialize, Serialize};

use async_trait::async_trait;

use crate::ids::{SandboxProfileId, WorkspaceProfileId};
use crate::sandbox::{Agent, Sandbox, Workspace};
use crate::{AgentId, SandboxId, SubjectId, WorkspaceId};

use super::{TriggerError, TriggerId};

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

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LaunchResolutionError {
    #[error("launch context is unavailable: {0}")]
    Unavailable(String),
    #[error("launch context is corrupt: {0}")]
    Corrupt(String),
}

#[async_trait]
pub trait LaunchContextReader: Send + Sync {
    /// Return only an Agent with the exact durable admission key or reuse
    /// receipt for this Launch. This never selects another matching Agent.
    async fn admitted_trigger_agent(
        &self,
        _launch: &super::Launch,
    ) -> Result<Option<Agent>, LaunchResolutionError> {
        Err(LaunchResolutionError::Unavailable(
            "committed Launch Agent discovery is unavailable".into(),
        ))
    }

    async fn owned_sandbox(
        &self,
        _owner: &SubjectId,
        _id: &SandboxId,
    ) -> Result<Option<Sandbox>, LaunchResolutionError> {
        Ok(None)
    }

    async fn owned_agent(
        &self,
        _owner: &SubjectId,
        _agent_id: &crate::AgentId,
    ) -> Result<Option<Agent>, LaunchResolutionError> {
        Ok(None)
    }

    async fn newest_matching_sandbox(
        &self,
        owner: &SubjectId,
        profile_id: &SandboxProfileId,
    ) -> Result<Option<Sandbox>, LaunchResolutionError>;

    async fn matching_workspace(
        &self,
        owner: &SubjectId,
        sandbox_id: &SandboxId,
        profile_id: &WorkspaceProfileId,
    ) -> Result<Option<Workspace>, LaunchResolutionError>;

    async fn newest_live_trigger_agent(
        &self,
        owner: &SubjectId,
        trigger_id: &TriggerId,
    ) -> Result<Option<Agent>, LaunchResolutionError>;
}
