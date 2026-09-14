use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use layer2_core::{Capability, SandboxId, ServiceId};

use super::idempotency::{IdempotencyKey, RequestBody};
use super::instances::{execute_command, HierarchyState};
use super::instances_openapi_shapes::{GrantSandboxServiceAccessRequest, SandboxView};
use super::instances_problem::HierarchyProblem;
use super::BodyProblem;
use crate::AuthenticatedCaller;

#[utoipa::path(
    post, path = "/api/v1/sandboxes/{id}/service-access", tag = "sandboxes",
    params(("id" = String, Path), ("Idempotency-Key" = String, Header)),
    request_body = GrantSandboxServiceAccessRequest,
    responses(
        (status = 200, description = "Explicit owner grant recorded or already present", body = SandboxView),
        (status = 400, body = crate::rest::ProblemDocument),
        (status = 401, body = crate::rest::ProblemDocument),
        (status = 403, body = crate::rest::ProblemDocument),
        (status = 404, body = crate::rest::ProblemDocument),
        (status = 409, description = "New service access requires a fully paused Sandbox, or command is in progress", body = crate::rest::ProblemDocument),
        (status = 413, body = crate::rest::ProblemDocument),
        (status = 415, body = crate::rest::ProblemDocument),
        (status = 422, description = "Invalid services or reused idempotency key", body = crate::rest::ProblemDocument),
        (status = 503, body = crate::rest::ProblemDocument)
    )
)]
pub(super) async fn grant(
    State(state): State<HierarchyState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
    key: IdempotencyKey,
    body: Result<RequestBody<GrantSandboxServiceAccessRequest>, BodyProblem>,
) -> Response {
    let command_state = state.clone();
    let command_caller = caller.clone();
    execute_command(
        &state,
        &caller,
        Capability::UseOwnSandboxes,
        key,
        format!("sandbox:service-access:{id}"),
        body,
        move |body| async move {
            let services = body
                .services
                .into_iter()
                .map(ServiceId::new)
                .collect::<Vec<_>>();
            let sandbox = command_state
                .sandboxes
                .grant_service_access(&command_caller, &SandboxId(id), &services)
                .await
                .map_err(HierarchyProblem::sandbox)?;
            Ok((StatusCode::OK, Json(SandboxView::from(&sandbox))).into_response())
        },
    )
    .await
}
