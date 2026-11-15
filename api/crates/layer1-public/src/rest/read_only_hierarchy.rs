//! Finite personal hierarchy reads for an explicitly inert control plane.
use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::get,
    Json, Router,
};
use layer2_core::{
    sandbox::{store::PageRequest, HierarchyReadService, OwnerSandboxQuery, SandboxStatus},
    SandboxId,
};
use serde::Deserialize;

use super::{
    instances_openapi_shapes::{
        OwnerSandboxDetailView, OwnerSandboxPageView, OwnerSandboxSummaryView, WorkspaceListView,
    },
    instances_problem::HierarchyProblem,
};
use crate::AuthenticatedCaller;

/// Does not construct write services, provider adapters or background workers.
pub fn read_only_hierarchy_router(service: Arc<HierarchyReadService>) -> Router {
    Router::new()
        .route("/api/v1/sandboxes", get(list))
        .route("/api/v1/sandboxes/{id}", get(read))
        .route("/api/v1/sandboxes/{id}/workspaces", get(workspaces))
        .fallback(|| async { StatusCode::SERVICE_UNAVAILABLE })
        .with_state(service)
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct QueryParams {
    status: Option<SandboxStatus>,
    #[serde(default)]
    exclude_retired: bool,
    cursor: Option<String>,
    limit: Option<u16>,
    organization: Option<String>,
    #[serde(default)]
    shared: bool,
}

async fn list(
    State(service): State<Arc<HierarchyReadService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    params: Result<Query<QueryParams>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<OwnerSandboxPageView>, HierarchyProblem> {
    let params = params.map_err(|_| HierarchyProblem::InvalidQuery)?.0;
    if params.shared || params.organization.is_some() {
        return Err(HierarchyProblem::Unavailable);
    }
    if params.cursor.as_ref().is_some_and(String::is_empty) {
        return Err(HierarchyProblem::InvalidPage);
    }
    let page = PageRequest::new(params.limit.unwrap_or(100), params.cursor)
        .map_err(|_| HierarchyProblem::InvalidPage)?;
    let page = service
        .list_sandboxes(
            &caller,
            &OwnerSandboxQuery {
                status: params.status,
                exclude_retired: params.exclude_retired,
                page,
            },
        )
        .await
        .map_err(HierarchyProblem::sandbox_read)?;
    Ok(Json(OwnerSandboxPageView {
        items: page
            .items
            .into_iter()
            .map(OwnerSandboxSummaryView::from)
            .collect(),
        next: page.next,
    }))
}

async fn read(
    State(service): State<Arc<HierarchyReadService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
) -> Result<Json<OwnerSandboxDetailView>, HierarchyProblem> {
    service
        .sandbox(&caller, &SandboxId(id))
        .await
        .map(Into::into)
        .map(Json)
        .map_err(HierarchyProblem::sandbox_read)
}

async fn workspaces(
    State(service): State<Arc<HierarchyReadService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
) -> Result<Json<WorkspaceListView>, HierarchyProblem> {
    let items = service
        .workspaces(&caller, &SandboxId(id))
        .await
        .map_err(HierarchyProblem::sandbox_read)?;
    Ok(Json(WorkspaceListView {
        items: items.into_iter().map(Into::into).collect(),
    }))
}

#[cfg(test)]
mod tests;
