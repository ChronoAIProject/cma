//! Owner-facing Worktree reads.

use axum::{extract::Path, extract::State, routing::get, Json, Router};
use layer2_core::{WorkspaceId, WorktreeId};

use crate::{AuthenticatedCaller, AuthenticatedResourceCaller, DelegatedCredential};
use layer2_core::sandbox::{WorkspaceHierarchyReading, WorktreeHierarchyReading};

use super::hierarchy_shared_shapes::{AccessibleWorktreeListView, AccessibleWorktreeView};
use super::instances::HierarchyState;
use super::instances_problem::HierarchyProblem;

pub(super) fn routes() -> Router<HierarchyState> {
    Router::new()
        .route("/api/v1/workspaces/{id}/worktrees", get(list))
        .route("/api/v1/worktrees/{id}", get(read))
}

#[utoipa::path(get, path = "/api/v1/workspaces/{id}/worktrees", tag = "worktrees", params(("id" = String, Path)), responses((status = 200, body = AccessibleWorktreeListView), (status = 404, body = crate::rest::ProblemDocument)))]
pub(super) async fn list(
    State(state): State<HierarchyState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    resource: Option<AuthenticatedResourceCaller>,
    Path(id): Path<String>,
) -> Result<Json<AccessibleWorktreeListView>, HierarchyProblem> {
    let id = WorkspaceId(id);
    match resource {
        Some(resource) => {
            state
                .hierarchy
                .workspace_authorized(
                    &resource.context,
                    resource.credential.as_ref().map(DelegatedCredential::value),
                    &id,
                )
                .await
        }
        None => state
            .hierarchy
            .workspace(&caller, &id)
            .await
            .map(|value| WorkspaceHierarchyReading::Personal(Box::new(value))),
    }
    .map(Into::into)
    .map(Json)
    .map_err(HierarchyProblem::workspace_read)
}

#[utoipa::path(get, path = "/api/v1/worktrees/{id}", tag = "worktrees", params(("id" = String, Path)), responses((status = 200, body = AccessibleWorktreeView), (status = 404, body = crate::rest::ProblemDocument)))]
pub(super) async fn read(
    State(state): State<HierarchyState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    resource: Option<AuthenticatedResourceCaller>,
    Path(id): Path<String>,
) -> Result<Json<AccessibleWorktreeView>, HierarchyProblem> {
    let id = WorktreeId(id);
    match resource {
        Some(resource) => {
            state
                .hierarchy
                .worktree_authorized(
                    &resource.context,
                    resource.credential.as_ref().map(DelegatedCredential::value),
                    &id,
                )
                .await
        }
        None => state
            .hierarchy
            .worktree(&caller, &id)
            .await
            .map(WorktreeHierarchyReading::Personal),
    }
    .map(Into::into)
    .map(Json)
    .map_err(HierarchyProblem::worktree_read)
}
