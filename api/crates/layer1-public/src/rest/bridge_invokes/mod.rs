use super::{
    idempotency::{IdempotencyKey, RequestBody},
    BodyProblem,
};
use crate::{AuthenticatedResourceCaller, DelegatedCredential};
use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use layer2_core::bridge::{invoke::*, BridgeId};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
mod problem;
pub(crate) mod shapes;
use shapes::*;

pub fn bridge_invoke_router(service: Option<Arc<BridgeInvokeService>>) -> Router {
    Router::new()
        .route("/api/v1/bridges/{id}/invoke-grants", get(list).post(issue))
        .route("/api/v1/bridges/{id}/invoke-readiness", get(readiness))
        .route("/api/v1/bridge-invoke-grants/{id}", get(detail))
        .route("/api/v1/bridge-invoke-grants/{id}/revoke", post(revoke))
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
fn respond<T: Serialize>(result: Result<T, BridgeInvokeError>, status: StatusCode) -> Response {
    match result {
        Ok(value) => (status, Json(value)).into_response(),
        Err(error) => problem::InvokeProblem(error).into_response(),
    }
}
fn unavailable() -> Response {
    problem::InvokeProblem(BridgeInvokeError::Unavailable).into_response()
}
fn body<T>(body: Result<RequestBody<T>, BodyProblem>) -> Result<T, Box<Response>> {
    body.and_then(|body| body.into_parts().0)
        .map_err(|error| Box::new(error.into_response()))
}
fn key(key: IdempotencyKey) -> Result<String, Box<Response>> {
    key.into_required_value()
        .map_err(|error| Box::new(error.into_response()))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ListParams {
    cursor: Option<String>,
    limit: Option<u32>,
}

#[utoipa::path(get,path="/api/v1/bridges/{id}/invoke-grants",tag="bridges",params(("id"=String,Path),("cursor"=Option<String>,Query),("limit"=Option<u32>,Query)),responses((status=200,body=InvokePageShape),(status=404,body=super::ProblemDocument),(status=503,body=super::ProblemDocument)))]
pub(crate) async fn list(
    State(service): State<Option<Arc<BridgeInvokeService>>>,
    caller: AuthenticatedResourceCaller,
    Path(id): Path<String>,
    params: Result<Query<ListParams>, axum::extract::rejection::QueryRejection>,
) -> Response {
    let Some(service) = service else {
        return unavailable();
    };
    let Ok(Query(params)) = params else {
        return problem::InvokeProblem(BridgeInvokeError::Invalid("query")).into_response();
    };
    let cursor = params.cursor.map(BridgeInvokeGrantId);
    respond(
        service
            .list(
                &caller.context,
                credential(&caller),
                &BridgeId(id),
                cursor.as_ref(),
                params.limit.unwrap_or(50),
            )
            .await,
        StatusCode::OK,
    )
}
#[utoipa::path(get,path="/api/v1/bridges/{id}/invoke-readiness",tag="bridges",params(("id"=String,Path)),responses((status=200,body=InvokeReadinessShape),(status=404,body=super::ProblemDocument),(status=503,body=super::ProblemDocument)))]
pub(crate) async fn readiness(
    State(service): State<Option<Arc<BridgeInvokeService>>>,
    caller: AuthenticatedResourceCaller,
    Path(id): Path<String>,
) -> Response {
    let Some(service) = service else {
        return unavailable();
    };
    respond(
        service
            .readiness(&caller.context, credential(&caller), &BridgeId(id))
            .await,
        StatusCode::OK,
    )
}
#[utoipa::path(get,path="/api/v1/bridge-invoke-grants/{id}",tag="bridges",params(("id"=String,Path)),responses((status=200,body=InvokeGrantShape),(status=404,body=super::ProblemDocument),(status=503,body=super::ProblemDocument)))]
pub(crate) async fn detail(
    State(service): State<Option<Arc<BridgeInvokeService>>>,
    caller: AuthenticatedResourceCaller,
    Path(id): Path<String>,
) -> Response {
    let Some(service) = service else {
        return unavailable();
    };
    respond(
        service
            .detail(
                &caller.context,
                credential(&caller),
                &BridgeInvokeGrantId(id),
            )
            .await,
        StatusCode::OK,
    )
}
#[utoipa::path(post,path="/api/v1/bridges/{id}/invoke-grants",tag="bridges",params(("id"=String,Path),("Idempotency-Key"=String,Header)),request_body=IssueInvokeGrantRequest,responses((status=201,body=InvokeReceiptShape),(status=400,body=super::ProblemDocument),(status=403,body=super::ProblemDocument),(status=409,body=super::ProblemDocument),(status=503,body=super::ProblemDocument)))]
pub(crate) async fn issue(
    State(service): State<Option<Arc<BridgeInvokeService>>>,
    caller: AuthenticatedResourceCaller,
    Path(id): Path<String>,
    idempotency: IdempotencyKey,
    input: Result<RequestBody<IssueInvokeGrantRequest>, BodyProblem>,
) -> Response {
    let Some(service) = service else {
        return unavailable();
    };
    let key = match key(idempotency) {
        Ok(key) => key,
        Err(response) => return *response,
    };
    let input = match body(input) {
        Ok(input) => input,
        Err(response) => return *response,
    };
    respond(
        service
            .issue(
                &caller.context,
                credential(&caller),
                IssueBridgeInvokeGrant {
                    bridge_id: BridgeId(id),
                    expected_bridge_revision: input.expected_bridge_revision,
                    target: input.target,
                    shared_context: input.shared_context,
                    idempotency_key: key,
                },
            )
            .await,
        StatusCode::CREATED,
    )
}
#[utoipa::path(post,path="/api/v1/bridge-invoke-grants/{id}/revoke",tag="bridges",params(("id"=String,Path),("Idempotency-Key"=String,Header)),request_body=RevokeInvokeGrantRequest,responses((status=200,body=InvokeReceiptShape),(status=400,body=super::ProblemDocument),(status=403,body=super::ProblemDocument),(status=409,body=super::ProblemDocument),(status=503,body=super::ProblemDocument)))]
pub(crate) async fn revoke(
    State(service): State<Option<Arc<BridgeInvokeService>>>,
    caller: AuthenticatedResourceCaller,
    Path(id): Path<String>,
    idempotency: IdempotencyKey,
    input: Result<RequestBody<RevokeInvokeGrantRequest>, BodyProblem>,
) -> Response {
    let Some(service) = service else {
        return unavailable();
    };
    let key = match key(idempotency) {
        Ok(key) => key,
        Err(response) => return *response,
    };
    let input = match body(input) {
        Ok(input) => input,
        Err(response) => return *response,
    };
    respond(
        service
            .revoke(
                &caller.context,
                credential(&caller),
                RevokeBridgeInvokeGrant {
                    grant_id: BridgeInvokeGrantId(id),
                    expected_bridge_revision: input.expected_bridge_revision,
                    expected_revision: input.expected_revision,
                    idempotency_key: key,
                },
            )
            .await,
        StatusCode::OK,
    )
}
