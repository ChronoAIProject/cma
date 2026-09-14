use layer2_core::sandbox::{AgentDetailReading, AgentStatus, SharedAgentDetail};
use serde::Serialize;
use utoipa::ToSchema;

use super::agent_openapi_shapes::AgentView;

/// Personal owners retain the established full detail document. All other
/// authorized readers receive the explicit Agent-only shared projection.
#[derive(Debug, Serialize, ToSchema)]
#[serde(untagged)]
pub(crate) enum AccessibleAgentView {
    Personal(Box<AgentView>),
    Shared(SharedAgentView),
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
enum SharedAccess {
    Shared,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct SharedAgentView {
    access: SharedAccess,
    id: String,
    #[schema(value_type = String)]
    status: AgentStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    created_at: String,
    updated_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    ready_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stopped_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    deleted_at: Option<String>,
}

impl From<AgentDetailReading> for AccessibleAgentView {
    fn from(value: AgentDetailReading) -> Self {
        match value {
            AgentDetailReading::Personal(lineage) => {
                Self::Personal(Box::new(AgentView::from_lineage(*lineage, None)))
            }
            AgentDetailReading::Shared(detail) => {
                let SharedAgentDetail {
                    id,
                    status,
                    title,
                    model,
                    created_at,
                    updated_at,
                    ready_at,
                    stopped_at,
                    deleted_at,
                } = *detail;
                Self::Shared(SharedAgentView {
                    access: SharedAccess::Shared,
                    id: id.to_string(),
                    status,
                    title,
                    model,
                    created_at,
                    updated_at,
                    ready_at,
                    stopped_at,
                    deleted_at,
                })
            }
        }
    }
}
