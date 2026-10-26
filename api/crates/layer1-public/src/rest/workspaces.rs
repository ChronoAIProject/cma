//! Owner-facing Workspace routes.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use layer2_core::profiles::WorkspaceProfileRef;
use layer2_core::sandbox::{CreateInlineWorkspace, CreateWorkspace};
use layer2_core::{Capability, SandboxId, WorkspaceId, WorkspaceProfileId};

use super::hierarchy_shared_shapes::{AccessibleWorkspaceDetailView, AccessibleWorkspaceListView};
use crate::{AuthenticatedCaller, AuthenticatedResourceCaller, DelegatedCredential};
use layer2_core::sandbox::{SandboxHierarchyReading, WorkspaceHierarchyReading};

use super::idempotency::{operation_scope, IdempotencyKey, RequestBody};
use super::instances::{confirmed_digest, execute_command, execute_empty_command, HierarchyState};
use super::instances_openapi_shapes::{
    CreateInlineWorkspaceRequest, CreateWorkspaceRequest, WorkspaceView,
};
use super::instances_problem::HierarchyProblem;
use super::BodyProblem;

pub(super) fn routes() -> Router<HierarchyState> {
    Router::new()
        .route("/api/v1/sandboxes/{id}/workspaces", post(create).get(list))
        .route(
            "/api/v1/sandboxes/{id}/workspaces/inline",
            post(create_inline),
        )
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

#[utoipa::path(
    post,
    path = "/api/v1/sandboxes/{id}/workspaces/inline",
    tag = "workspaces",
    params(("id" = String, Path), ("Idempotency-Key" = String, Header)),
    request_body = CreateInlineWorkspaceRequest,
    responses(
        (status = 201, body = WorkspaceView),
        (status = 409, body = crate::rest::ProblemDocument),
        (status = 422, body = crate::rest::ProblemDocument),
        (status = 429, body = crate::rest::ProblemDocument)
    )
)]
pub(super) async fn create_inline(
    State(state): State<HierarchyState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
    key: IdempotencyKey,
    body: Result<RequestBody<CreateInlineWorkspaceRequest>, BodyProblem>,
) -> Response {
    let command_state = state.clone();
    let command_caller = caller.clone();
    execute_command(
        &state,
        &caller,
        Capability::UseOwnSandboxes,
        key,
        operation_scope("sandbox:workspace:inline-create", &[&id]),
        body,
        move |body| async move {
            let workspace = command_state
                .workspaces
                .create_inline(
                    &command_caller,
                    &SandboxId(id),
                    CreateInlineWorkspace {
                        source: body.source.into(),
                        name: body.name,
                    },
                )
                .await
                .map_err(HierarchyProblem::workspace)?;
            Ok((StatusCode::CREATED, Json(WorkspaceView::from(&workspace))).into_response())
        },
    )
    .await
}

#[utoipa::path(get, path = "/api/v1/sandboxes/{id}/workspaces", tag = "workspaces", params(("id" = String, Path)), responses((status = 200, body = AccessibleWorkspaceListView), (status = 404, body = crate::rest::ProblemDocument)))]
pub(super) async fn list(
    State(state): State<HierarchyState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    resource: Option<AuthenticatedResourceCaller>,
    Path(id): Path<String>,
) -> Result<Json<AccessibleWorkspaceListView>, HierarchyProblem> {
    let id = SandboxId(id);
    match resource {
        Some(resource) => {
            state
                .hierarchy
                .sandbox_authorized(
                    &resource.context,
                    resource.credential.as_ref().map(DelegatedCredential::value),
                    &id,
                )
                .await
        }
        None => state
            .hierarchy
            .sandbox(&caller, &id)
            .await
            .map(|value| SandboxHierarchyReading::Personal(Box::new(value))),
    }
    .map(Into::into)
    .map(Json)
    .map_err(HierarchyProblem::sandbox_read)
}

#[utoipa::path(get, path = "/api/v1/workspaces/{id}", tag = "workspaces", params(("id" = String, Path)), responses((status = 200, body = AccessibleWorkspaceDetailView), (status = 404, body = crate::rest::ProblemDocument)))]
pub(super) async fn read(
    State(state): State<HierarchyState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    resource: Option<AuthenticatedResourceCaller>,
    Path(id): Path<String>,
) -> Result<Json<AccessibleWorkspaceDetailView>, HierarchyProblem> {
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
