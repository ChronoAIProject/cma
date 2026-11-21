use layer2_core::sandbox::{AgentDetailReading, AgentStatus, SharedAgentDetail};
use serde::Serialize;
use utoipa::ToSchema;

use super::agent_openapi_shapes::{AgentListItemView, AgentView};

/// Personal owners retain the established full detail document. All other
/// authorized readers receive the explicit Agent-only shared projection.
#[derive(Debug, Serialize, ToSchema)]
#[serde(untagged)]
pub(crate) enum AccessibleAgentView {
    Personal(Box<AgentView>),
    Shared(Box<SharedAgentView>),
}

impl AccessibleAgentView {
    pub(super) fn with_agent_page(mut self, origin: &crate::agent_page::AgentPageOrigin) -> Self {
        match &mut self {
            Self::Personal(view) => view.callback_url = origin.agent_url(&view.id),
            Self::Shared(view) => view.callback_url = origin.agent_url(&view.id),
        }
        self
    }
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
enum SharedAccess {
    Shared,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct SharedAgentView {
    /// Browser page for this CMA Agent.
    #[serde(skip_serializing_if = "Option::is_none")]
    callback_url: Option<String>,
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
                Self::Shared(Box::new(SharedAgentView {
                    callback_url: None,
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
                }))
            }
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(untagged)]
pub(crate) enum AccessibleAgentListItemView {
    Personal(Box<AgentListItemView>),
    Shared(Box<SharedAgentView>),
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct AccessibleAgentPageView {
    pub(crate) items: Vec<AccessibleAgentListItemView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) next: Option<String>,
}

impl From<AgentDetailReading> for AccessibleAgentListItemView {
    fn from(value: AgentDetailReading) -> Self {
        match value {
            AgentDetailReading::Personal(lineage) => {
                Self::Personal(Box::new(AgentListItemView::from(lineage.agent)))
            }
            AgentDetailReading::Shared(value) => {
                let AccessibleAgentView::Shared(value) =
                    AccessibleAgentView::from(AgentDetailReading::Shared(value))
                else {
                    unreachable!("explicit shared projection")
                };
                Self::Shared(value)
            }
        }
    }
}
