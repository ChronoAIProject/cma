//! Owner-controlled automatic-pause whitelist route.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use layer2_core::{Capability, SandboxId};
use serde::Deserialize;
use utoipa::ToSchema;

use super::idempotency::{IdempotencyKey, RequestBody};
use super::instances::{execute_command, HierarchyState};
use super::instances_openapi_shapes::SandboxView;
use super::instances_problem::HierarchyProblem;
use super::BodyProblem;
use crate::AuthenticatedCaller;

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct AlwaysOnRequest {
    pub(crate) always_on: bool,
    pub(crate) expected_revision: u64,
}

#[utoipa::path(
    post,
    path = "/api/v1/sandboxes/{id}/always-on",
    tag = "sandboxes",
    params(("id" = String, Path), ("Idempotency-Key" = String, Header)),
    request_body = AlwaysOnRequest,
    responses(
        (status = 200, body = SandboxView),
        (status = 400, body = crate::rest::ProblemDocument),
        (status = 403, body = crate::rest::ProblemDocument),
        (status = 404, body = crate::rest::ProblemDocument),
        (status = 409, body = crate::rest::ProblemDocument),
        (status = 422, body = crate::rest::ProblemDocument)
    )
)]
pub(super) async fn always_on(
    State(state): State<HierarchyState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
    key: IdempotencyKey,
    body: Result<RequestBody<AlwaysOnRequest>, BodyProblem>,
) -> Response {
    let command_state = state.clone();
    let command_caller = caller.clone();
    execute_command(
        &state,
        &caller,
        Capability::UseOwnSandboxes,
        key,
        format!("sandbox:always-on:{id}"),
        body,
        move |body| async move {
            let sandbox = command_state
                .sandboxes
                .set_always_on(
                    &command_caller,
                    &SandboxId(id),
                    body.expected_revision,
                    body.always_on,
                )
                .await
                .map_err(HierarchyProblem::sandbox)?;
            Ok((StatusCode::OK, Json(SandboxView::from(&sandbox))).into_response())
        },
    )
    .await
}
