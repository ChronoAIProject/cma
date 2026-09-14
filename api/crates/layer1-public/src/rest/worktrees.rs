//! Owner-facing Worktree reads.

use axum::{extract::Path, extract::State, routing::get, Json, Router};
use layer2_core::{WorkspaceId, WorktreeId};

use crate::AuthenticatedCaller;

use super::instances::HierarchyState;
use super::instances_openapi_shapes::{WorktreeListView, WorktreeView};
use super::instances_problem::HierarchyProblem;

pub(super) fn routes() -> Router<HierarchyState> {
    Router::new()
        .route("/api/v1/workspaces/{id}/worktrees", get(list))
        .route("/api/v1/worktrees/{id}", get(read))
}

#[utoipa::path(get, path = "/api/v1/workspaces/{id}/worktrees", tag = "worktrees", params(("id" = String, Path)), responses((status = 200, body = WorktreeListView), (status = 404, body = crate::rest::ProblemDocument)))]
pub(super) async fn list(
    State(state): State<HierarchyState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
) -> Result<Json<WorktreeListView>, HierarchyProblem> {
    state
        .hierarchy
        .worktrees(&caller, &WorkspaceId(id))
        .await
        .map(|items| {
            Json(WorktreeListView {
                items: items.into_iter().map(Into::into).collect(),
            })
        })
        .map_err(HierarchyProblem::workspace_read)
}

#[utoipa::path(get, path = "/api/v1/worktrees/{id}", tag = "worktrees", params(("id" = String, Path)), responses((status = 200, body = WorktreeView), (status = 404, body = crate::rest::ProblemDocument)))]
pub(super) async fn read(
    State(state): State<HierarchyState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
) -> Result<Json<WorktreeView>, HierarchyProblem> {
    state
        .hierarchy
        .worktree(&caller, &WorktreeId(id))
        .await
        .map(Into::into)
        .map(Json)
        .map_err(HierarchyProblem::worktree_read)
}
