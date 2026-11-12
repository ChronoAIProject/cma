//! Public Event Worker Profile management; the web uses these same operations.
mod shapes;
use crate::AuthenticatedCaller;
use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use layer2_core::bots::BotError;
use layer2_core::event_workers::*;
use shapes::*;
use std::sync::Arc;

pub fn router(service: Arc<EventWorkerProfileService>) -> Router {
    Router::new()
        .route("/api/v1/event-worker-profiles", get(list).post(create))
        .route(
            "/api/v1/event-worker-profiles/{id}",
            get(read).put(update).delete(retire),
        )
        .route(
            "/api/v1/event-worker-profiles/{id}/revisions/{revision}",
            get(revision),
        )
        .with_state(service)
        .layer(axum::middleware::from_fn(no_store))
}
async fn no_store(request: axum::extract::Request, next: axum::middleware::Next) -> Response {
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    response
}
#[derive(serde::Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
pub struct ListQuery {
    after: Option<String>,
    limit: Option<usize>,
}
#[utoipa::path(get,path="/api/v1/event-worker-profiles",tag="event-worker-profiles",params(ListQuery),responses((status=200,body=ProfilePage),(status=403,body=super::ProblemDocument),(status=503,body=super::ProblemDocument)))]
pub async fn list(
    State(service): State<Arc<EventWorkerProfileService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    query: Result<Query<ListQuery>, axum::extract::rejection::QueryRejection>,
) -> Response {
    let Query(query) = match query {
        Ok(value) => value,
        Err(_) => return problem(BotError::Invalid("invalid query".into())),
    };
    let limit = query.limit.unwrap_or(50);
    match service.list(&caller, query.after.as_deref(), limit).await {
        Ok(rows) => Json(ProfilePage {
            next_after: if rows.len() == limit {
                rows.last().map(|v| v.id.clone())
            } else {
                None
            },
            items: rows.into_iter().map(Into::into).collect(),
        })
        .into_response(),
        Err(e) => problem(e),
    }
}
#[utoipa::path(post,path="/api/v1/event-worker-profiles",tag="event-worker-profiles",params(("Idempotency-Key"=String,Header)),request_body=ProfileInput,responses((status=201,body=ProfileView),(status=400,body=super::ProblemDocument),(status=403,body=super::ProblemDocument),(status=409,body=super::ProblemDocument),(status=503,body=super::ProblemDocument)))]
pub async fn create(
    State(service): State<Arc<EventWorkerProfileService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    headers: HeaderMap,
    body: Result<Json<ProfileInput>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Json(body) = match body {
        Ok(v) => v,
        Err(e) => return super::BodyProblem::from(e).into_response(),
    };
    let keys: Vec<_> = headers.get_all("idempotency-key").iter().collect();
    let key = match keys.as_slice() {
        [key] => key.to_str().unwrap_or_default(),
        _ => "",
    };
    view(
        service.create(&caller, key, body.into()).await,
        StatusCode::CREATED,
    )
}
#[utoipa::path(get,path="/api/v1/event-worker-profiles/{id}",tag="event-worker-profiles",params(("id"=String,Path)),responses((status=200,body=ProfileView),(status=404,body=super::ProblemDocument)))]
pub async fn read(
    State(service): State<Arc<EventWorkerProfileService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
) -> Response {
    view(service.get(&caller, &id, None).await, StatusCode::OK)
}
#[utoipa::path(get,path="/api/v1/event-worker-profiles/{id}/revisions/{revision}",tag="event-worker-profiles",params(("id"=String,Path),("revision"=u32,Path)),responses((status=200,body=ProfileView),(status=404,body=super::ProblemDocument)))]
pub async fn revision(
    State(service): State<Arc<EventWorkerProfileService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    path: Result<Path<(String, u32)>, axum::extract::rejection::PathRejection>,
) -> Response {
    let Path((id, revision)) = match path {
        Ok(value) => value,
        Err(_) => return problem(BotError::Invalid("invalid revision".into())),
    };
    view(
        service.get(&caller, &id, Some(revision)).await,
        StatusCode::OK,
    )
}
#[utoipa::path(put,path="/api/v1/event-worker-profiles/{id}",tag="event-worker-profiles",params(("id"=String,Path)),request_body=UpdateInput,responses((status=200,body=ProfileView),(status=404,body=super::ProblemDocument),(status=409,body=super::ProblemDocument)))]
pub async fn update(
    State(service): State<Arc<EventWorkerProfileService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
    body: Result<Json<UpdateInput>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Json(body) = match body {
        Ok(v) => v,
        Err(e) => return super::BodyProblem::from(e).into_response(),
    };
    view(
        service
            .update(&caller, &id, body.expected_revision, body.source.into())
            .await,
        StatusCode::OK,
    )
}
#[derive(serde::Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
pub struct DeleteQuery {
    expected_revision: u32,
}
#[utoipa::path(delete,path="/api/v1/event-worker-profiles/{id}",tag="event-worker-profiles",params(("id"=String,Path),DeleteQuery),responses((status=200,body=ProfileView),(status=404,body=super::ProblemDocument),(status=409,body=super::ProblemDocument)))]
pub async fn retire(
    State(service): State<Arc<EventWorkerProfileService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
    query: Result<Query<DeleteQuery>, axum::extract::rejection::QueryRejection>,
) -> Response {
    let Query(query) = match query {
        Ok(value) => value,
        Err(_) => return problem(BotError::Invalid("invalid query".into())),
    };
    view(
        service.retire(&caller, &id, query.expected_revision).await,
        StatusCode::OK,
    )
}
fn view(value: Result<EventWorkerProfile, BotError>, status: StatusCode) -> Response {
    match value {
        Ok(v) => (status, Json(ProfileView::from(v))).into_response(),
        Err(e) => problem(e),
    }
}
fn problem(error: BotError) -> Response {
    let (status, code, detail) = match error {
        BotError::AccountDeletionInProgress => {
            return super::account_deletion_problem(
                layer2_core::account_deletion::AccountDeletionError::InProgress,
            )
        }
        BotError::Forbidden => (
            StatusCode::FORBIDDEN,
            "event_worker_profile_forbidden",
            "Profile management permission is required.",
        ),
        BotError::NotFound => (
            StatusCode::NOT_FOUND,
            "event_worker_profile_not_found",
            "Event Worker Profile or source revision was not found.",
        ),
        BotError::Conflict => (
            StatusCode::CONFLICT,
            "event_worker_profile_conflict",
            "The revision or idempotency key conflicts with existing work.",
        ),
        BotError::Invalid(_) => (
            StatusCode::BAD_REQUEST,
            "event_worker_profile_invalid",
            "Profile name, source revisions, or request parameters are invalid.",
        ),
        _ => (
            StatusCode::SERVICE_UNAVAILABLE,
            "event_worker_profile_unavailable",
            "Event Worker Profiles are temporarily unavailable.",
        ),
    };
    super::problems::problem_response(status, super::problem_document(status, code, detail, code))
}
