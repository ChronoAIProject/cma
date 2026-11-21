//! Agent inventory response shapes.

use super::agent_observation_shapes::AgentRuntimeMetadataView;
use super::agent_openapi_shapes::AgentProfileSummaryView;
use layer2_core::sandbox::{Agent, AgentStatus};
use serde::Serialize;
use utoipa::ToSchema;

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct AgentListItemView {
    #[serde(skip_serializing_if = "Option::is_none")]
    runtime_metadata: Option<AgentRuntimeMetadataView>,
    id: String,
    sandbox_id: String,
    workspace_id: String,
    worktree_id: String,
    #[schema(value_type = String)]
    status: AgentStatus,
    profile: AgentProfileSummaryView,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    owner_title: Option<String>,
    created_at: String,
    updated_at: String,
}

impl From<Agent> for AgentListItemView {
    fn from(agent: Agent) -> Self {
        Self {
            id: agent.id.to_string(),
            sandbox_id: agent.sandbox_id.to_string(),
            workspace_id: agent.workspace_id.to_string(),
            worktree_id: agent.worktree_id.to_string(),
            status: agent.status,
            profile: (&agent).into(),
            runtime_metadata: AgentRuntimeMetadataView::from_agent(&agent),
            owner_title: agent.conversation.preferences.title,
            title: agent.title,
            created_at: agent.created_at,
            updated_at: agent.updated_at,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct AgentPageView {
    pub(crate) items: Vec<AgentListItemView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) next: Option<String>,
}
