//! The durable Agent list contract shared by both control-plane modes.

use std::sync::Arc;

use axum::{
    extract::{Query, State},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use layer2_core::{
    sandbox::{
        store::{Page, PageRequest},
        Agent, AgentListQuery, AgentReadError, AgentStatus,
    },
    Caller, SandboxId, SubjectId, WorkspaceId,
};
use serde::Deserialize;

use crate::{
    rest::{
        agent_openapi_shapes::AgentListItemView,
        agent_shared_shapes::{AccessibleAgentListItemView, AccessibleAgentPageView},
        instance_problem::InstanceProblem,
    },
    AuthenticatedCaller,
};

const DEFAULT_PAGE_LIMIT: u16 = 200;

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(deny_unknown_fields)]
pub(crate) struct AgentPageParams {
    organization: Option<String>,
    sandbox: Option<String>,
    workspace: Option<String>,
    #[param(value_type = Option<String>)]
    status: Option<AgentStatus>,
    cursor: Option<String>,
    #[param(minimum = 1, maximum = 200, default = 200)]
    limit: Option<u16>,
}

impl AgentPageParams {
    pub(super) fn into_parts(
        self,
    ) -> Result<(Option<SubjectId>, AgentListQuery, PageRequest), InstanceProblem> {
        if self.organization.as_ref().is_some_and(|value| {
            value.trim().is_empty() || value.len() > 128 || value != value.trim()
        }) || self.sandbox.as_ref().is_some_and(|value| value.is_empty())
            || self
                .workspace
                .as_ref()
                .is_some_and(|value| value.is_empty())
            || self.cursor.as_ref().is_some_and(String::is_empty)
        {
            return Err(InstanceProblem::InvalidQuery);
        }
        let page = PageRequest::new(self.limit.unwrap_or(DEFAULT_PAGE_LIMIT), self.cursor)
            .map_err(|_| InstanceProblem::InvalidQuery)?;
        Ok((
            self.organization.map(SubjectId::new),
            AgentListQuery {
                sandbox_id: self.sandbox.map(SandboxId),
                workspace_id: self.workspace.map(WorkspaceId),
                status: self.status,
            },
            page,
        ))
    }
}

/// Durable personal Agent inventory without any runtime service dependency.
#[async_trait::async_trait]
pub trait PersonalAgentInventory: Send + Sync {
    async fn list(
        &self,
        caller: &Caller,
        query: &AgentListQuery,
        page: &PageRequest,
    ) -> Result<Page<Agent>, AgentReadError>;
}

pub fn personal_agent_inventory_router(inventory: Arc<dyn PersonalAgentInventory>) -> Router {
    Router::new()
        .route(
            "/api/v1/agents",
            get(list_personal).layer(axum::middleware::from_fn(
                super::agent_state::list_private_headers,
            )),
        )
        .with_state(inventory)
}

async fn list_personal(
    State(inventory): State<Arc<dyn PersonalAgentInventory>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    params: Result<Query<AgentPageParams>, axum::extract::rejection::QueryRejection>,
) -> Result<Response, InstanceProblem> {
    let (organization, query, page) = params
        .map_err(|_| InstanceProblem::InvalidQuery)?
        .0
        .into_parts()?;
    if organization.is_some() {
        return Err(InstanceProblem::Forbidden);
    }
    let stored = inventory.list(&caller, &query, &page).await?;
    Ok(personal_agent_page(stored))
}

pub(super) fn personal_agent_page(stored: Page<Agent>) -> Response {
    tracing::debug_span!(target: "cma.agent_inventory", "serialize").in_scope(|| {
        Json(AccessibleAgentPageView {
            items: stored
                .items
                .into_iter()
                .map(|agent| {
                    AccessibleAgentListItemView::Personal(Box::new(AgentListItemView::from(agent)))
                })
                .collect(),
            next: stored.next,
        })
        .into_response()
    })
}
