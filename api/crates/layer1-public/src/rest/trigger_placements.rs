//! Publisher-owned Trigger placement routes.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use layer2_core::trigger_domain::{PlacementId, TriggerId};

use crate::AuthenticatedCaller;

use super::idempotency::{IdempotencyKey, RequestBody};
use super::trigger_openapi_shapes::{CreatePlacementRequest, PlacementQuery, PlacementView};
use super::trigger_problem::TriggerProblem;
use super::triggers::{
    execute_command, execute_empty_command, require_owned_trigger, TriggerRoutesState,
};
use super::BodyProblem;

pub(super) fn routes() -> Router<TriggerRoutesState> {
    Router::new()
        .route("/api/v1/triggers/{id}/placements", post(create).get(list))
        .route(
            "/api/v1/triggers/{id}/placements/{placement_id}/retire",
            post(retire),
        )
}

#[utoipa::path(post, path = "/api/v1/triggers/{id}/placements", tag = "trigger placements", params(("id" = String, Path), ("Idempotency-Key" = String, Header)), request_body = CreatePlacementRequest, responses((status = 201, body = PlacementView), (status = 404, body = crate::rest::ProblemDocument), (status = 422, body = crate::rest::ProblemDocument)))]
pub(super) async fn create(
    State(state): State<TriggerRoutesState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
    key: IdempotencyKey,
    body: Result<RequestBody<CreatePlacementRequest>, BodyProblem>,
) -> Response {
    if let Err(problem) = require_owned_trigger(&state, &caller, &id).await {
        return problem.into_response();
    }
    let command_state = state.clone();
    let command_caller = caller.clone();
    let operation = format!("trigger:placement:create:{id}");
    execute_command(
        &state,
        &caller,
        key,
        operation,
        body,
        move |body| async move {
            let placement = command_state
                .triggers
                .create_placement(&command_caller, &TriggerId(id), body.source_url, body.label)
                .await?;
            Ok((StatusCode::CREATED, Json(PlacementView::from(placement))).into_response())
        },
    )
    .await
}

#[utoipa::path(get, path = "/api/v1/triggers/{id}/placements", tag = "trigger placements", params(("id" = String, Path), PlacementQuery), responses((status = 200, body = [PlacementView]), (status = 404, body = crate::rest::ProblemDocument)))]
pub(super) async fn list(
    State(state): State<TriggerRoutesState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
    query: Result<Query<PlacementQuery>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<Vec<PlacementView>>, TriggerProblem> {
    let query = query
        .map_err(|_| TriggerProblem::InvalidQuery("invalid_placement_query"))?
        .0;
    state
        .triggers
        .placements(&caller, &TriggerId(id), query.include_retired)
        .await
        .map(|items| Json(items.into_iter().map(Into::into).collect()))
        .map_err(Into::into)
}

#[utoipa::path(post, path = "/api/v1/triggers/{id}/placements/{placement_id}/retire", tag = "trigger placements", params(("id" = String, Path), ("placement_id" = String, Path), ("Idempotency-Key" = String, Header)), responses((status = 200, body = PlacementView), (status = 404, body = crate::rest::ProblemDocument)))]
pub(super) async fn retire(
    State(state): State<TriggerRoutesState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path((id, placement_id)): Path<(String, String)>,
    key: IdempotencyKey,
) -> Response {
    if let Err(problem) = require_owned_trigger(&state, &caller, &id).await {
        return problem.into_response();
    }
    let command_state = state.clone();
    let command_caller = caller.clone();
    let operation = format!("trigger:placement:retire:{id}:{placement_id}");
    execute_empty_command(&state, &caller, key, operation, move || async move {
        command_state
            .triggers
            .retire_placement(&command_caller, &TriggerId(id), &PlacementId(placement_id))
            .await
            .map(PlacementView::from)
            .map(Json)
            .map(IntoResponse::into_response)
            .map_err(Into::into)
    })
    .await
}
