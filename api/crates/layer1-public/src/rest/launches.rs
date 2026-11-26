//! Owner-facing Launch routes and public Trigger Launch admission.

use std::sync::Arc;

use axum::{
    extract::{Path, Query, RawQuery, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use layer2_core::trigger_domain::{LaunchServiceError, PublicTriggerId, MAX_LAUNCH_EVENT_LIMIT};
use layer2_core::{Capability, LaunchId};

use crate::{AuthenticatedCaller, AuthenticatedResourceCaller};

use super::idempotency::{self, IdempotencyKey, RequestBody};
use super::trigger_openapi_shapes::{
    CreateLaunchRequest, LaunchEventPageView, LaunchEventQuery, LaunchPageQuery, LaunchPageView,
    LaunchView, RequirementsSnapshotView, StartLaunchRequest,
};
use super::trigger_problem::TriggerProblem;
use super::triggers::{execute_command, execute_empty_command, TriggerRoutesState};
use super::BodyProblem;

pub(super) fn routes() -> Router<TriggerRoutesState> {
    Router::new()
        .route("/api/v1/triggers/public/{public_id}/launches", post(create))
        .route(
            "/api/v2/triggers/public/{public_id}/launches",
            post(super::launch_subject::create_bound),
        )
        .route("/api/v1/launches", get(list))
        .route("/api/v1/launches/{id}", get(read))
        .route("/api/v1/launches/{id}/events", get(events))
        .route("/api/v1/launches/{id}/requirements", get(requirements))
        .route("/api/v1/launches/{id}/start", post(start))
        .route("/api/v1/launches/{id}/cancel", post(cancel))
}

fn require_launcher(caller: &layer2_core::Caller) -> Result<(), TriggerProblem> {
    (caller.authorization.allows(Capability::LaunchTriggers)
        || caller.authorization.allows(Capability::CreateSandbox))
    .then_some(())
    .ok_or_else(|| TriggerProblem::from(LaunchServiceError::Forbidden))
}

pub(super) async fn require_owned_launch(
    state: &TriggerRoutesState,
    caller: &layer2_core::Caller,
    id: &str,
) -> Result<layer2_core::trigger_domain::Launch, TriggerProblem> {
    state
        .launches
        .read(caller, &LaunchId(id.to_owned()))
        .await
        .map_err(Into::into)
}

#[utoipa::path(
    post,
    path = "/api/v1/triggers/public/{public_id}/launches",
    tag = "trigger launches",
    params(("public_id" = String, Path), ("Idempotency-Key" = String, Header)),
    request_body = CreateLaunchRequest,
    responses(
        (status = 201, body = LaunchView),
        (status = 202, body = LaunchView),
        (status = 409, body = crate::rest::ProblemDocument),
        (status = 429, body = crate::rest::ProblemDocument)
    )
)]
pub(super) async fn create(
    State(state): State<TriggerRoutesState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    resource: Option<AuthenticatedResourceCaller>,
    Path(public_id): Path<String>,
    key: IdempotencyKey,
    body: Result<RequestBody<CreateLaunchRequest>, BodyProblem>,
) -> Response {
    if let Err(problem) = require_launcher(&caller) {
        return problem.into_response();
    }
    let operation = format!("trigger:launch:create:{public_id}");
    let (guard, body, command_key) = match idempotency::required_command(
        key,
        Arc::clone(&state.operations),
        &caller,
        operation,
        body,
    )
    .await
    {
        Ok(parts) => parts,
        Err(response) => return response,
    };
    let body = match body {
        Ok(body) => body,
        Err(error) => return guard.finish(Err(error.into_response())).await,
    };
    let permit = match state.launch_quota.reserve(&caller).await {
        Ok(permit) => permit,
        Err(error) => return guard.finish(Err(error.into_response())).await,
    };
    let request = body.into_domain(command_key);
    let public_id = PublicTriggerId(public_id);
    let result = match resource {
        Some(resource) => {
            state
                .launches
                .create_or_resume_as(&resource.context, &public_id, request)
                .await
        }
        None => {
            state
                .launches
                .create_or_resume(&caller, &public_id, request)
                .await
        }
    };
    let response = match result {
        Ok(result) => {
            permit.settle(&result);
            let status = if result.accepted() {
                StatusCode::ACCEPTED
            } else {
                StatusCode::CREATED
            };
            Ok((status, Json(LaunchView::from(result.launch))).into_response())
        }
        Err(error) => Err(TriggerProblem::from(error).into_response()),
    };
    guard.finish(response).await
}

#[utoipa::path(get, path = "/api/v1/launches", tag = "trigger launches", params(LaunchPageQuery), responses((status = 200, body = LaunchPageView)))]
pub(super) async fn list(
    State(state): State<TriggerRoutesState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    query: Result<Query<LaunchPageQuery>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<LaunchPageView>, TriggerProblem> {
    let query = query
        .map_err(|_| TriggerProblem::InvalidQuery("invalid_launch_cursor"))?
        .0;
    let page = state.launches.list(&caller, &query.into_domain()).await?;
    Ok(Json(LaunchPageView {
        items: page.items.into_iter().map(Into::into).collect(),
        next_cursor: page.next,
    }))
}

#[utoipa::path(get, path = "/api/v1/launches/{id}", tag = "trigger launches", params(("id" = String, Path)), responses((status = 200, body = LaunchView), (status = 404, body = crate::rest::ProblemDocument)))]
pub(super) async fn read(
    State(state): State<TriggerRoutesState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
) -> Result<Json<LaunchView>, TriggerProblem> {
    require_owned_launch(&state, &caller, &id)
        .await
        .map(LaunchView::from)
        .map(Json)
}

#[utoipa::path(get, path = "/api/v1/launches/{id}/events", tag = "trigger launches", params(("id" = String, Path), LaunchEventQuery), responses((status = 200, body = LaunchEventPageView), (status = 400, body = crate::rest::ProblemDocument), (status = 404, body = crate::rest::ProblemDocument)))]
pub(super) async fn events(
    State(state): State<TriggerRoutesState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
    RawQuery(raw_query): RawQuery,
) -> Result<Json<LaunchEventPageView>, TriggerProblem> {
    let launch = require_owned_launch(&state, &caller, &id).await?;
    let query: LaunchEventQuery = serde_urlencoded::from_str(raw_query.as_deref().unwrap_or(""))
        .map_err(|_| TriggerProblem::InvalidQuery("invalid_launch_event_query"))?;
    let after = query.after.unwrap_or(0);
    let limit = query
        .limit
        .unwrap_or(layer2_core::trigger_domain::DEFAULT_LAUNCH_EVENT_LIMIT);
    if !(1..=MAX_LAUNCH_EVENT_LIMIT).contains(&limit) {
        return Err(TriggerProblem::InvalidQuery("invalid_launch_event_query"));
    }
    if after > launch.event_head {
        return Err(TriggerProblem::CursorBeyondEnd {
            head: launch.event_head,
        });
    }
    let events = state
        .launches
        .events(&caller, &launch.id, after, Some(limit))
        .await?;
    let next_cursor = events.last().map_or(after, |event| event.seq);
    Ok(Json(LaunchEventPageView {
        launch_id: launch.id.to_string(),
        events: events.into_iter().map(Into::into).collect(),
        next_cursor,
    }))
}

#[utoipa::path(get, path = "/api/v1/launches/{id}/requirements", tag = "trigger launches", params(("id" = String, Path)), responses((status = 200, body = RequirementsSnapshotView), (status = 404, body = crate::rest::ProblemDocument)))]
pub(super) async fn requirements(
    State(state): State<TriggerRoutesState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
) -> Result<Json<RequirementsSnapshotView>, TriggerProblem> {
    let id = LaunchId(id);
    let snapshot = state.launches.requirements(&caller, &id).await?;
    let metadata = state
        .connections
        .describe_launch_services(&caller, &id)
        .await?;
    Ok(Json(
        RequirementsSnapshotView::from(snapshot).with_connection_metadata(metadata),
    ))
}

#[utoipa::path(post, path = "/api/v1/launches/{id}/start", tag = "trigger launches", params(("id" = String, Path), ("Idempotency-Key" = String, Header)), request_body = StartLaunchRequest, responses((status = 202, body = LaunchView), (status = 409, body = crate::rest::ProblemDocument)))]
pub(super) async fn start(
    State(state): State<TriggerRoutesState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
    key: IdempotencyKey,
    body: Result<RequestBody<StartLaunchRequest>, BodyProblem>,
) -> Response {
    if let Err(problem) = require_owned_launch(&state, &caller, &id).await {
        return problem.into_response();
    }
    let command_state = state.clone();
    let command_caller = caller.clone();
    let operation = format!("launch:start:{id}");
    execute_command(
        &state,
        &caller,
        key,
        operation,
        body,
        move |body| async move {
            let launch = command_state
                .launches
                .start(&command_caller, &LaunchId(id), body.into())
                .await?;
            Ok((StatusCode::ACCEPTED, Json(LaunchView::from(launch))).into_response())
        },
    )
    .await
}

#[utoipa::path(post, path = "/api/v1/launches/{id}/cancel", tag = "trigger launches", params(("id" = String, Path), ("Idempotency-Key" = String, Header)), responses((status = 200, body = LaunchView), (status = 409, body = crate::rest::ProblemDocument)))]
pub(super) async fn cancel(
    State(state): State<TriggerRoutesState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
    key: IdempotencyKey,
) -> Response {
    if let Err(problem) = require_owned_launch(&state, &caller, &id).await {
        return problem.into_response();
    }
    let command_state = state.clone();
    let command_caller = caller.clone();
    let operation = format!("launch:cancel:{id}");
    execute_empty_command(&state, &caller, key, operation, move || async move {
        command_state
            .launches
            .cancel(&command_caller, &LaunchId(id))
            .await
            .map(LaunchView::from)
            .map(Json)
            .map(IntoResponse::into_response)
            .map_err(Into::into)
    })
    .await
}
