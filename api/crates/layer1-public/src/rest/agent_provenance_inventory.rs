use std::sync::Arc;

use axum::{
    extract::{Query, State},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use layer2_core::sandbox::store::PageRequest;
use layer2_core::sandbox::{
    AgentCreationCategory, AgentListQuery, AgentProvenanceInventoryQuery,
    AgentProvenanceInventoryRow, AgentProvenanceInventoryService, AgentStatus,
};
use layer2_core::{SandboxId, SubjectId, WorkspaceId};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::{
    agent_provenance::AgentCreationCategoryView, resource_access_problem::ResourceProblem,
};
use crate::{AuthenticatedCaller, AuthenticatedResourceCaller, DelegatedCredential};

pub fn agent_provenance_inventory_router(service: Arc<AgentProvenanceInventoryService>) -> Router {
    Router::new()
        .route("/api/v1/agents/provenance", get(list))
        .with_state(service)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Params {
    task_status: Option<layer2_core::sandbox::AgentTaskStatus>,
    organization: Option<String>,
    #[serde(default)]
    shared: bool,
    sandbox: Option<String>,
    workspace: Option<String>,
    status: Option<AgentStatus>,
    creation_origin: Option<AgentCreationCategory>,
    channel_active: Option<bool>,
    channel_history: Option<bool>,
    cursor: Option<String>,
    limit: Option<u16>,
}

#[derive(Serialize, ToSchema)]
pub(crate) struct AgentProvenanceInventoryItemView {
    #[schema(value_type = String)]
    task_status: layer2_core::sandbox::AgentTaskStatus,
    agent_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[schema(value_type = String)]
    status: AgentStatus,
    created_at: String,
    updated_at: String,
    creation_origin: AgentCreationCategoryView,
    channel_usage: super::agent_channel_usage::ChannelUsageSummaryView,
}

#[derive(Serialize, ToSchema)]
pub(crate) struct AgentProvenanceInventoryPageView {
    items: Vec<AgentProvenanceInventoryItemView>,
    /// Continue even after an empty filtered page. This cursor represents the
    /// last consumed authorized candidate, which need not match the filter.
    #[serde(skip_serializing_if = "Option::is_none")]
    next: Option<String>,
}

#[utoipa::path(get, path = "/api/v1/agents/provenance", tag = "agents",
    params(("shared" = Option<bool>, Query, description = "Explicit incoming shares across current organizations; mutually exclusive with organization"), ("organization" = Option<String>, Query), ("sandbox" = Option<String>, Query),
        ("workspace" = Option<String>, Query), ("status" = Option<String>, Query), ("task_status" = Option<String>, Query),
        ("creation_origin" = Option<AgentCreationCategoryView>, Query),
        ("channel_active" = Option<bool>, Query), ("channel_history" = Option<bool>, Query),
        ("cursor" = Option<String>, Query), ("limit" = Option<u16>, Query)),
    responses((status = 200, body = AgentProvenanceInventoryPageView),
        (status = 400, body = crate::rest::ProblemDocument), (status = 404, body = crate::rest::ProblemDocument)))]
pub(crate) async fn list(
    State(service): State<Arc<AgentProvenanceInventoryService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    resource: Option<AuthenticatedResourceCaller>,
    params: Result<Query<Params>, axum::extract::rejection::QueryRejection>,
) -> Response {
    let Ok(Query(params)) = params else {
        return invalid();
    };
    let Ok(page) = PageRequest::new(params.limit.unwrap_or(50), params.cursor) else {
        return invalid();
    };
    let query = AgentProvenanceInventoryQuery {
        task_status: params.task_status,
        organization: params.organization.map(SubjectId::new),
        shared: params.shared,
        agents: AgentListQuery {
            sandbox_id: params.sandbox.map(SandboxId),
            workspace_id: params.workspace.map(WorkspaceId),
            status: params.status,
        },
        creation_origin: params.creation_origin,
        channel_active: params.channel_active,
        channel_history: params.channel_history,
    };
    let result = match resource {
        Some(resource) => {
            service
                .list(
                    &resource.context,
                    resource.credential.as_ref().map(DelegatedCredential::value),
                    &query,
                    &page,
                )
                .await
        }
        None => service.list_personal(&caller, &query, &page).await,
    };
    match result {
        Ok(page) => Json(AgentProvenanceInventoryPageView {
            items: page.items.into_iter().map(Into::into).collect(),
            next: page.next,
        })
        .into_response(),
        Err(layer2_core::resource_access::ResourceAccessError::Store(
            layer2_core::resource_access::ResourceAccessStoreError::Invalid,
        )) => invalid(),
        Err(error) => ResourceProblem(error).into_response(),
    }
}

fn invalid() -> Response {
    super::instance_problem::InstanceProblem::InvalidQuery.into_response()
}

impl From<AgentProvenanceInventoryRow> for AgentProvenanceInventoryItemView {
    fn from(value: AgentProvenanceInventoryRow) -> Self {
        Self {
            task_status: value.task_status,
            agent_id: value.agent_id.to_string(),
            title: value.title,
            status: value.status,
            created_at: value.created_at,
            updated_at: value.updated_at,
            creation_origin: value.creation_origin.into(),
            channel_usage: value.channel_usage.into(),
        }
    }
}
