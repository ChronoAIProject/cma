//! Owner-facing NyxID service-connection handoff routes.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use layer2_core::ServiceHandoffId;

use crate::AuthenticatedCaller;

use super::idempotency::{IdempotencyKey, RequestBody};
use super::launches::require_owned_launch;
use super::trigger_openapi_shapes::{
    CreateServiceConnectionRequest, CreatedServiceConnectionView, ServiceConnectionStatusView,
};
use super::trigger_problem::TriggerProblem;
use super::triggers::{execute_command, TriggerRoutesState};
use super::BodyProblem;

pub(super) fn routes() -> Router<TriggerRoutesState> {
    Router::new()
        .route("/api/v1/service-connections", post(create))
        .route("/api/v1/service-connections/{handoff_id}", get(read))
}

#[utoipa::path(post, path = "/api/v1/service-connections", tag = "service connections", params(("Idempotency-Key" = String, Header)), request_body = CreateServiceConnectionRequest, responses((status = 201, body = CreatedServiceConnectionView), (status = 404, body = crate::rest::ProblemDocument), (status = 409, body = crate::rest::ProblemDocument)))]
pub(super) async fn create(
    State(state): State<TriggerRoutesState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    key: IdempotencyKey,
    body: Result<RequestBody<CreateServiceConnectionRequest>, BodyProblem>,
) -> Response {
    if let Ok(body) = &body {
        if let Some(request) = body.value() {
            if let Err(problem) = require_owned_launch(&state, &caller, &request.launch_id).await {
                return problem.for_connection_launch_preflight().into_response();
            }
        }
    }
    let command_state = state.clone();
    let command_caller = caller.clone();
    execute_command(
        &state,
        &caller,
        key,
        "service-connection:create".into(),
        body,
        move |body| async move {
            let handoff = command_state
                .connections
                .create(&command_caller, body.into())
                .await?;
            Ok((
                StatusCode::CREATED,
                Json(CreatedServiceConnectionView::from(handoff)),
            )
                .into_response())
        },
    )
    .await
}

#[utoipa::path(get, path = "/api/v1/service-connections/{handoff_id}", tag = "service connections", params(("handoff_id" = String, Path)), responses((status = 200, body = ServiceConnectionStatusView), (status = 404, body = crate::rest::ProblemDocument)))]
pub(super) async fn read(
    State(state): State<TriggerRoutesState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(handoff_id): Path<String>,
) -> Result<Json<ServiceConnectionStatusView>, TriggerProblem> {
    state
        .connections
        .read_and_refresh(&caller, &ServiceHandoffId(handoff_id))
        .await
        .map(ServiceConnectionStatusView::from)
        .map(Json)
        .map_err(Into::into)
}
