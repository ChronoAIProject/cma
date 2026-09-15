//! Explicit child cleanup and durable progress through public management REST.
use super::{
    idempotency::{IdempotencyKey, RequestBody},
    BodyProblem,
};
use crate::{AuthenticatedResourceCaller, DelegatedCredential};
use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use layer2_core::bridge::{cleanup::*, BridgeId};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
mod problem;
mod shapes;
use shapes::*;

pub fn bridge_cleanup_router(service: Option<Arc<BridgeCleanupService>>) -> Router {
    Router::new()
        .route("/api/v1/bridges/{id}/allocations", get(list))
        .route(
            "/api/v1/bridges/{id}/allocations/{allocation}/child-cleanup",
            get(detail).post(submit),
        )
        .layer(axum::middleware::from_fn(private_headers))
        .layer(axum::extract::DefaultBodyLimit::max(8192))
        .with_state(service)
}
async fn private_headers(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
}
fn credential(caller: &AuthenticatedResourceCaller) -> Option<&str> {
    caller.credential.as_ref().map(DelegatedCredential::value)
}
fn respond<T: Serialize>(status: StatusCode, result: Result<T, CleanupError>) -> Response {
    match result {
        Ok(value) => (status, Json(value)).into_response(),
        Err(error) => problem::CleanupProblem(error).into_response(),
    }
}
fn unavailable() -> Response {
    problem::CleanupProblem(CleanupError::Unavailable).into_response()
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ListParams {
    cursor: Option<String>,
    limit: Option<u32>,
}

#[utoipa::path(get,path="/api/v1/bridges/{id}/allocations",tag="bridges",params(("id"=String,Path),("cursor"=Option<String>,Query),("limit"=Option<u32>,Query)),responses((status=200,body=CleanupAllocationPageShape),(status=400,body=super::ProblemDocument),(status=403,body=super::ProblemDocument),(status=404,body=super::ProblemDocument),(status=409,body=super::ProblemDocument),(status=503,body=super::ProblemDocument)))]
pub(crate) async fn list(
    State(service): State<Option<Arc<BridgeCleanupService>>>,
    caller: AuthenticatedResourceCaller,
    Path(id): Path<String>,
    params: Result<Query<ListParams>, axum::extract::rejection::QueryRejection>,
) -> Response {
    let Some(service) = service else {
        return unavailable();
    };
    let Ok(Query(params)) = params else {
        return problem::CleanupProblem(CleanupError::Invalid).into_response();
    };
    respond(
        StatusCode::OK,
        service
            .allocations(
                &caller.context,
                credential(&caller),
                &BridgeId(id),
                params.cursor.as_deref(),
                params.limit.unwrap_or(50),
            )
            .await,
    )
}
#[utoipa::path(get,path="/api/v1/bridges/{id}/allocations/{allocation}/child-cleanup",tag="bridges",params(("id"=String,Path),("allocation"=String,Path)),responses((status=200,body=ChildCleanupStatusShape),(status=400,body=super::ProblemDocument),(status=403,body=super::ProblemDocument),(status=404,body=super::ProblemDocument),(status=409,body=super::ProblemDocument),(status=503,body=super::ProblemDocument)))]
pub(crate) async fn detail(
    State(service): State<Option<Arc<BridgeCleanupService>>>,
    caller: AuthenticatedResourceCaller,
    Path((id, allocation)): Path<(String, String)>,
) -> Response {
    let Some(service) = service else {
        return unavailable();
    };
    respond(
        StatusCode::OK,
        service
            .status(
                &caller.context,
                credential(&caller),
                &BridgeId(id),
                &allocation,
            )
            .await,
    )
}
#[utoipa::path(post,path="/api/v1/bridges/{id}/allocations/{allocation}/child-cleanup",tag="bridges",params(("id"=String,Path),("allocation"=String,Path),("Idempotency-Key"=String,Header)),request_body=ChildCleanupRequestShape,responses((status=202,body=ChildCleanupStatusShape),(status=400,body=super::ProblemDocument),(status=403,body=super::ProblemDocument),(status=404,body=super::ProblemDocument),(status=409,body=super::ProblemDocument),(status=415,body=super::ProblemDocument),(status=422,body=super::ProblemDocument),(status=503,body=super::ProblemDocument)))]
pub(crate) async fn submit(
    State(service): State<Option<Arc<BridgeCleanupService>>>,
    caller: AuthenticatedResourceCaller,
    Path((id, allocation_id)): Path<(String, String)>,
    key: IdempotencyKey,
    input: Result<RequestBody<ChildCleanupRequest>, BodyProblem>,
) -> Response {
    let Some(service) = service else {
        return unavailable();
    };
    let idempotency_key = match key.into_required_value() {
        Ok(key) => key,
        Err(error) => return error.into_response(),
    };
    let request = match input.and_then(|body| body.into_parts().0) {
        Ok(input) => input,
        Err(error) => return error.into_response(),
    };
    respond(
        StatusCode::ACCEPTED,
        service
            .submit(
                &caller.context,
                credential(&caller),
                &BridgeId(id),
                ChildCleanupCommand {
                    allocation_id,
                    idempotency_key,
                    request,
                },
            )
            .await,
    )
}
