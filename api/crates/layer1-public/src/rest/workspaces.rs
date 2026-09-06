//! Owner-facing Workspace routes.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use layer2_core::profiles::WorkspaceProfileRef;
use layer2_core::sandbox::CreateWorkspace;
use layer2_core::{Capability, SandboxId, WorkspaceId, WorkspaceProfileId};

use crate::AuthenticatedCaller;

use super::idempotency::{operation_scope, IdempotencyKey, RequestBody};
use super::instances::{confirmed_digest, execute_command, execute_empty_command, HierarchyState};
use super::instances_openapi_shapes::{
    CreateWorkspaceRequest, WorkspaceDetailView, WorkspaceListView, WorkspaceView,
};
use super::instances_problem::HierarchyProblem;
use super::BodyProblem;

pub(super) fn routes() -> Router<HierarchyState> {
    Router::new()
        .route("/api/v1/sandboxes/{id}/workspaces", post(create).get(list))
        .route("/api/v1/workspaces/{id}", get(read).delete(remove))
}

#[utoipa::path(
    post,
    path = "/api/v1/sandboxes/{id}/workspaces",
    tag = "workspaces",
    params(("id" = String, Path), ("Idempotency-Key" = String, Header)),
    request_body = CreateWorkspaceRequest,
    responses(
        (status = 201, body = WorkspaceView),
        (status = 409, body = crate::rest::ProblemDocument),
        (status = 429, body = crate::rest::ProblemDocument)
    )
)]
pub(super) async fn create(
    State(state): State<HierarchyState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
    key: IdempotencyKey,
    body: Result<RequestBody<CreateWorkspaceRequest>, BodyProblem>,
) -> Response {
    let command_state = state.clone();
    let command_caller = caller.clone();
    execute_command(
        &state,
        &caller,
        Capability::UseOwnSandboxes,
        key,
        operation_scope("sandbox:workspace:create", &[&id]),
        body,
        move |body| async move {
            let confirmed_digest = confirmed_digest(body.trust_confirmed, body.commands_digest)?;
            let workspace = command_state
                .workspaces
                .create(
                    &command_caller,
                    &SandboxId(id),
                    CreateWorkspace {
                        profile: WorkspaceProfileRef::new(
                            WorkspaceProfileId(body.workspace_profile.id),
                            body.workspace_profile.revision,
                        ),
                        confirmed_digest,
                    },
                )
                .await
                .map_err(HierarchyProblem::workspace)?;
            Ok((StatusCode::CREATED, Json(WorkspaceView::from(&workspace))).into_response())
        },
    )
    .await
}

#[utoipa::path(get, path = "/api/v1/sandboxes/{id}/workspaces", tag = "workspaces", params(("id" = String, Path)), responses((status = 200, body = WorkspaceListView), (status = 404, body = crate::rest::ProblemDocument)))]
pub(super) async fn list(
    State(state): State<HierarchyState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
) -> Result<Json<WorkspaceListView>, HierarchyProblem> {
    state
        .hierarchy
        .workspaces(&caller, &SandboxId(id))
        .await
        .map(|items| {
            Json(WorkspaceListView {
                items: items.into_iter().map(Into::into).collect(),
            })
        })
        .map_err(HierarchyProblem::sandbox_read)
}

#[utoipa::path(get, path = "/api/v1/workspaces/{id}", tag = "workspaces", params(("id" = String, Path)), responses((status = 200, body = WorkspaceDetailView), (status = 404, body = crate::rest::ProblemDocument)))]
pub(super) async fn read(
    State(state): State<HierarchyState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
) -> Result<Json<WorkspaceDetailView>, HierarchyProblem> {
    state
        .hierarchy
        .workspace(&caller, &WorkspaceId(id))
        .await
        .map(Into::into)
        .map(Json)
        .map_err(HierarchyProblem::workspace_read)
}

#[utoipa::path(delete, path = "/api/v1/workspaces/{id}", tag = "workspaces", params(("id" = String, Path), ("Idempotency-Key" = String, Header)), responses((status = 202, body = WorkspaceView), (status = 409, body = crate::rest::ProblemDocument), (status = 404, body = crate::rest::ProblemDocument)))]
pub(super) async fn remove(
    State(state): State<HierarchyState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
    key: IdempotencyKey,
) -> Response {
    let command_state = state.clone();
    let command_caller = caller.clone();
    execute_empty_command(
        &state,
        &caller,
        Capability::UseOwnSandboxes,
        key,
        format!("workspace:delete:{id}"),
        move || async move {
            let workspace = command_state
                .workspaces
                .remove(&command_caller, &WorkspaceId(id))
                .await
                .map_err(HierarchyProblem::workspace)?;
            Ok((StatusCode::ACCEPTED, Json(WorkspaceView::from(&workspace))).into_response())
        },
    )
    .await
}
