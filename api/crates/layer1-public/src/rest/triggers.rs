//! Publisher-owned Trigger routes and anonymous public preview.

use std::{future::Future, sync::Arc};

use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use layer2_core::trigger_domain::{
    LaunchService, ServiceConnectionService, TriggerContent, TriggerId, TriggerPageRequest,
    TriggerService, TriggerServiceError,
};
use layer2_core::{Caller, Capability, OperationStore};

use crate::{AuthenticatedCaller, DelegatedCredential};

use super::idempotency::{self, IdempotencyKey, RequestBody};
use super::trigger_openapi_shapes::{
    CreateTriggerRequest, PublishTriggerRequest, PublishedTriggerRevisionView, TriggerContentBody,
    TriggerPageQuery, TriggerPageView, TriggerPreviewView, TriggerRevisionView,
    TriggerStatisticsView, TriggerView, UpdateTriggerRequest,
};
use super::trigger_problem::TriggerProblem;
use super::BodyProblem;
use super::LaunchQuota;

#[derive(Clone)]
pub struct TriggerRoutesState {
    pub(super) triggers: Arc<TriggerService>,
    pub(super) launches: Arc<LaunchService>,
    pub(super) connections: Arc<ServiceConnectionService>,
    pub(super) operations: Arc<dyn OperationStore>,
    pub(super) launch_quota: Arc<dyn LaunchQuota>,
}

impl TriggerRoutesState {
    pub fn new(
        triggers: Arc<TriggerService>,
        launches: Arc<LaunchService>,
        connections: Arc<ServiceConnectionService>,
        operations: Arc<dyn OperationStore>,
        launch_quota: Arc<dyn LaunchQuota>,
    ) -> Self {
        Self {
            triggers,
            launches,
            connections,
            operations,
            launch_quota,
        }
    }
}

pub(super) fn routes() -> Router<TriggerRoutesState> {
    Router::new()
        .route("/api/v1/triggers", post(create).get(list))
        .route(
            "/api/v1/triggers/{id}",
            get(read).put(update).delete(delete_trigger),
        )
        .route("/api/v1/triggers/{id}/publish", post(publish))
        .route("/api/v1/triggers/{id}/retire", post(retire))
        .route("/api/v1/triggers/{id}/revisions", get(revisions))
        .route("/api/v1/triggers/{id}/stats", get(statistics))
        .route("/api/v1/triggers/public/{public_id}", get(preview))
}

pub fn trigger_router(state: TriggerRoutesState) -> Router {
    routes()
        .merge(super::trigger_placements::routes())
        .merge(super::launches::routes())
        .merge(super::service_connections::routes())
        .with_state(state)
}

pub(super) fn require_publisher(caller: &Caller) -> Result<(), TriggerProblem> {
    caller
        .authorization
        .allows(Capability::PublishTriggers)
        .then_some(())
        .ok_or_else(|| {
            TriggerProblem::from(layer2_core::trigger_domain::TriggerServiceError::Forbidden)
        })
}

pub(super) async fn require_owned_trigger(
    state: &TriggerRoutesState,
    caller: &Caller,
    id: &str,
) -> Result<(), TriggerProblem> {
    state
        .triggers
        .read(caller, &TriggerId(id.to_owned()))
        .await
        .map(|_| ())
        .map_err(Into::into)
}

pub(super) async fn execute_command<T, F, Fut>(
    state: &TriggerRoutesState,
    caller: &Caller,
    key: IdempotencyKey,
    operation: String,
    body: Result<RequestBody<T>, BodyProblem>,
    execute: F,
) -> Response
where
    T: serde::de::DeserializeOwned,
    F: FnOnce(T) -> Fut,
    Fut: Future<Output = Result<Response, TriggerProblem>>,
{
    let (guard, body, _) = match idempotency::required_command(
        key,
        Arc::clone(&state.operations),
        caller,
        operation,
        body,
    )
    .await
    {
        Ok(parts) => parts,
        Err(response) => return response,
    };
    let result: Result<Response, Response> = match body {
        Ok(body) => execute(body).await.map_err(IntoResponse::into_response),
        Err(error) => Err(error.into_response()),
    };
    guard.finish(result).await
}

pub(super) async fn execute_empty_command<F, Fut>(
    state: &TriggerRoutesState,
    caller: &Caller,
    key: IdempotencyKey,
    operation: String,
    execute: F,
) -> Response
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<Response, TriggerProblem>>,
{
    let guard = match idempotency::required_empty_command(
        key,
        Arc::clone(&state.operations),
        caller,
        operation,
    )
    .await
    {
        Ok(guard) => guard,
        Err(response) => return response,
    };
    guard.finish(execute().await).await
}

fn reject_custom_image_selection(
    content: Option<&TriggerContentBody>,
) -> Result<(), TriggerProblem> {
    let Some(content) = content else {
        return Ok(());
    };
    TriggerContent::from(content.clone())
        .reject_custom_image_selection()
        .map_err(|error| TriggerProblem::from(TriggerServiceError::Reference(error)))
}

#[utoipa::path(
    post,
    path = "/api/v1/triggers",
    tag = "triggers",
    params(("Idempotency-Key" = String, Header)),
    request_body = CreateTriggerRequest,
    responses((status = 201, body = TriggerView), (status = 422, body = crate::rest::ProblemDocument))
)]
pub(super) async fn create(
    State(state): State<TriggerRoutesState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    credential: Option<axum::extract::Extension<DelegatedCredential>>,
    key: IdempotencyKey,
    body: Result<RequestBody<CreateTriggerRequest>, BodyProblem>,
) -> Response {
    if let Err(problem) = require_publisher(&caller) {
        return problem.into_response();
    }
    if let Err(problem) = reject_custom_image_selection(
        body.as_ref()
            .ok()
            .and_then(RequestBody::value)
            .map(|body| &body.content),
    ) {
        return problem.into_response();
    }
    let command_state = state.clone();
    let command_caller = caller.clone();
    let delegated_credential = credential.map(|value| value.value().to_owned());
    execute_command(
        &state,
        &caller,
        key,
        "trigger:create".into(),
        body,
        move |body| async move {
            let trigger = command_state
                .triggers
                .create_with_credential(
                    &command_caller,
                    body.content.into(),
                    delegated_credential.as_deref(),
                )
                .await?;
            Ok((StatusCode::CREATED, Json(TriggerView::from(trigger))).into_response())
        },
    )
    .await
}

#[utoipa::path(get, path = "/api/v1/triggers", tag = "triggers", params(TriggerPageQuery), responses((status = 200, body = TriggerPageView)))]
pub(super) async fn list(
    State(state): State<TriggerRoutesState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    query: Result<Query<TriggerPageQuery>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<TriggerPageView>, TriggerProblem> {
    require_publisher(&caller)?;
    let query = query
        .map_err(|_| TriggerProblem::InvalidQuery("invalid_trigger_page"))?
        .0;
    let page = state
        .triggers
        .list(
            &caller,
            &TriggerPageRequest {
                limit: query
                    .limit
                    .unwrap_or(layer2_core::trigger_domain::store::DEFAULT_TRIGGER_PAGE_SIZE),
                cursor: query.cursor,
            },
        )
        .await?;
    Ok(Json(TriggerPageView {
        items: page.items.into_iter().map(Into::into).collect(),
        next_cursor: page.next,
    }))
}

#[utoipa::path(get, path = "/api/v1/triggers/{id}", tag = "triggers", params(("id" = String, Path)), responses((status = 200, body = TriggerView), (status = 404, body = crate::rest::ProblemDocument)))]
pub(super) async fn read(
    State(state): State<TriggerRoutesState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
) -> Result<Json<TriggerView>, TriggerProblem> {
    state
        .triggers
        .read(&caller, &TriggerId(id))
        .await
        .map(TriggerView::from)
        .map(Json)
        .map_err(Into::into)
}

#[utoipa::path(put, path = "/api/v1/triggers/{id}", tag = "triggers", params(("id" = String, Path), ("Idempotency-Key" = String, Header)), request_body = UpdateTriggerRequest, responses((status = 200, body = TriggerView), (status = 409, body = crate::rest::ProblemDocument), (status = 422, body = crate::rest::ProblemDocument)))]
pub(super) async fn update(
    State(state): State<TriggerRoutesState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
    credential: Option<axum::extract::Extension<DelegatedCredential>>,
    key: IdempotencyKey,
    body: Result<RequestBody<UpdateTriggerRequest>, BodyProblem>,
) -> Response {
    if let Err(problem) = require_owned_trigger(&state, &caller, &id).await {
        return problem.into_response();
    }
    if let Err(problem) = reject_custom_image_selection(
        body.as_ref()
            .ok()
            .and_then(RequestBody::value)
            .map(|body| &body.content),
    ) {
        return problem.into_response();
    }
    let command_state = state.clone();
    let command_caller = caller.clone();
    let delegated_credential = credential.map(|value| value.value().to_owned());
    let operation = format!("trigger:update:{id}");
    execute_command(
        &state,
        &caller,
        key,
        operation,
        body,
        move |body| async move {
            command_state
                .triggers
                .update_draft_with_credential(
                    &command_caller,
                    &TriggerId(id),
                    body.draft_version,
                    body.content.into(),
                    delegated_credential.as_deref(),
                )
                .await
                .map(TriggerView::from)
                .map(Json)
                .map(IntoResponse::into_response)
                .map_err(Into::into)
        },
    )
    .await
}

#[utoipa::path(delete, path = "/api/v1/triggers/{id}", tag = "triggers", params(("id" = String, Path), ("Idempotency-Key" = String, Header)), responses((status = 204), (status = 404, body = crate::rest::ProblemDocument), (status = 409, body = crate::rest::ProblemDocument)))]
pub(super) async fn delete_trigger(
    State(state): State<TriggerRoutesState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
    key: IdempotencyKey,
) -> Response {
    // A successful delete removes the owner-visible record, so its exact
    // operation receipt must be allowed to replay before another owner read.
    if let Err(problem) = require_publisher(&caller) {
        return problem.into_response();
    }
    let command_state = state.clone();
    let command_caller = caller.clone();
    let operation = format!("trigger:delete:{id}");
    execute_empty_command(&state, &caller, key, operation, move || async move {
        command_state
            .triggers
            .delete(&command_caller, &TriggerId(id))
            .await?;
        Ok(StatusCode::NO_CONTENT.into_response())
    })
    .await
}

#[utoipa::path(post, path = "/api/v1/triggers/{id}/publish", tag = "triggers", params(("id" = String, Path), ("Idempotency-Key" = String, Header)), request_body = PublishTriggerRequest, responses((status = 201, body = PublishedTriggerRevisionView), (status = 409, body = crate::rest::ProblemDocument), (status = 422, body = crate::rest::ProblemDocument)))]
pub(super) async fn publish(
    State(state): State<TriggerRoutesState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
    key: IdempotencyKey,
    body: Result<RequestBody<PublishTriggerRequest>, BodyProblem>,
) -> Response {
    if let Err(problem) = require_owned_trigger(&state, &caller, &id).await {
        return problem.into_response();
    }
    let command_state = state.clone();
    let command_caller = caller.clone();
    let operation = format!("trigger:publish:{id}");
    execute_command(
        &state,
        &caller,
        key,
        operation,
        body,
        move |body| async move {
            let published = command_state
                .triggers
                .publish(&command_caller, &TriggerId(id), body.draft_version)
                .await?;
            Ok((
                StatusCode::CREATED,
                Json(PublishedTriggerRevisionView::from(published)),
            )
                .into_response())
        },
    )
    .await
}

#[utoipa::path(post, path = "/api/v1/triggers/{id}/retire", tag = "triggers", params(("id" = String, Path), ("Idempotency-Key" = String, Header)), responses((status = 200, body = TriggerView), (status = 409, body = crate::rest::ProblemDocument)))]
pub(super) async fn retire(
    State(state): State<TriggerRoutesState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
    key: IdempotencyKey,
) -> Response {
    if let Err(problem) = require_owned_trigger(&state, &caller, &id).await {
        return problem.into_response();
    }
    let command_state = state.clone();
    let command_caller = caller.clone();
    let operation = format!("trigger:retire:{id}");
    execute_empty_command(&state, &caller, key, operation, move || async move {
        command_state
            .triggers
            .retire(&command_caller, &TriggerId(id))
            .await
            .map(TriggerView::from)
            .map(Json)
            .map(IntoResponse::into_response)
            .map_err(Into::into)
    })
    .await
}

#[utoipa::path(get, path = "/api/v1/triggers/{id}/revisions", tag = "triggers", params(("id" = String, Path)), responses((status = 200, body = [TriggerRevisionView]), (status = 404, body = crate::rest::ProblemDocument)))]
pub(super) async fn revisions(
    State(state): State<TriggerRoutesState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
) -> Result<Json<Vec<TriggerRevisionView>>, TriggerProblem> {
    state
        .triggers
        .revisions(&caller, &TriggerId(id))
        .await
        .map(|items| Json(items.into_iter().map(Into::into).collect()))
        .map_err(Into::into)
}

#[utoipa::path(get, path = "/api/v1/triggers/{id}/stats", tag = "triggers", params(("id" = String, Path)), responses((status = 200, body = TriggerStatisticsView), (status = 404, body = crate::rest::ProblemDocument)))]
pub(super) async fn statistics(
    State(state): State<TriggerRoutesState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
) -> Result<Json<TriggerStatisticsView>, TriggerProblem> {
    state
        .triggers
        .statistics(&caller, &TriggerId(id))
        .await
        .map(TriggerStatisticsView::from)
        .map(Json)
        .map_err(Into::into)
}

#[utoipa::path(get, path = "/api/v1/triggers/public/{public_id}", tag = "triggers", params(("public_id" = String, Path)), responses((status = 200, body = TriggerPreviewView), (status = 404, body = crate::rest::ProblemDocument)), security(()))]
pub(super) async fn preview(
    State(state): State<TriggerRoutesState>,
    Path(public_id): Path<String>,
) -> Response {
    let result = state
        .triggers
        .preview(&layer2_core::trigger_domain::PublicTriggerId(public_id))
        .await
        .map(TriggerPreviewView::from)
        .map(Json)
        .map(IntoResponse::into_response)
        .map_err(TriggerProblem::from);
    let mut response = result.unwrap_or_else(IntoResponse::into_response);
    if matches!(response.status(), StatusCode::OK | StatusCode::NOT_FOUND) {
        response.headers_mut().insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("public, max-age=60"),
        );
        response.headers_mut().insert(
            header::REFERRER_POLICY,
            HeaderValue::from_static("no-referrer"),
        );
    }
    response
}
