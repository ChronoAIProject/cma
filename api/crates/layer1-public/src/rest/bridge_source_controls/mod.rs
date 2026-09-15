//! Explicit source-sender consent. This surface does not execute Agent controls.
use super::{
    idempotency::{IdempotencyKey, RequestBody},
    BodyProblem,
};
use crate::{AuthenticatedResourceCaller, DelegatedCredential};
use axum::{
    extract::{Path, State},
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use layer2_core::bridge::invoke::*;
use serde::Serialize;
use std::sync::Arc;
mod problem;
mod shapes;
use shapes::*;

pub fn bridge_source_control_router(service: Option<Arc<BridgeSourceControlService>>) -> Router {
    Router::new()
        .route(
            "/api/v1/bridge-invoke-grants/{id}/source-controls",
            get(detail).put(submit),
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
fn respond<T: Serialize>(result: Result<T, BridgeInvokeError>) -> Response {
    match result {
        Ok(value) => (StatusCode::OK, Json(value)).into_response(),
        Err(error) => problem::SourceControlProblem(error).into_response(),
    }
}
#[derive(Serialize)]
struct PolicyState {
    policy: Option<SourceControlPolicyView>,
}

/// Read separately recorded consent. An absent policy grants no sender controls.
#[utoipa::path(get,path="/api/v1/bridge-invoke-grants/{id}/source-controls",tag="bridges",params(("id"=String,Path)),responses((status=200,body=SourceControlStateShape),(status=400,body=super::ProblemDocument),(status=401,body=super::ProblemDocument),(status=403,body=super::ProblemDocument),(status=404,body=super::ProblemDocument),(status=409,body=super::ProblemDocument),(status=503,body=super::ProblemDocument)))]
pub(crate) async fn detail(
    State(service): State<Option<Arc<BridgeSourceControlService>>>,
    caller: AuthenticatedResourceCaller,
    Path(id): Path<String>,
) -> Response {
    let Some(service) = service else {
        return problem::SourceControlProblem(BridgeInvokeError::Unavailable).into_response();
    };
    respond(
        service
            .policy(
                &caller.context,
                caller.credential.as_ref().map(DelegatedCredential::value),
                &BridgeInvokeGrantId(id),
            )
            .await
            .map(|policy| PolicyState { policy }),
    )
}

/// Record or revoke explicit consent using the original key and exact revisions.
/// Enabled consent alone does not demonstrate runtime or connector support.
#[utoipa::path(put,path="/api/v1/bridge-invoke-grants/{id}/source-controls",tag="bridges",params(("id"=String,Path),("Idempotency-Key"=String,Header)),request_body=SourceControlRequestShape,responses((status=200,body=SourceControlReceiptShape),(status=400,body=super::ProblemDocument),(status=401,body=super::ProblemDocument),(status=403,body=super::ProblemDocument),(status=404,body=super::ProblemDocument),(status=409,body=super::ProblemDocument),(status=413,body=super::ProblemDocument),(status=415,body=super::ProblemDocument),(status=422,body=super::ProblemDocument),(status=503,body=super::ProblemDocument)))]
pub(crate) async fn submit(
    State(service): State<Option<Arc<BridgeSourceControlService>>>,
    caller: AuthenticatedResourceCaller,
    Path(id): Path<String>,
    key: IdempotencyKey,
    body: Result<RequestBody<SourceControlRequest>, BodyProblem>,
) -> Response {
    let Some(service) = service else {
        return problem::SourceControlProblem(BridgeInvokeError::Unavailable).into_response();
    };
    let key = match key.into_required_value() {
        Ok(key) => key,
        Err(error) => return error.into_response(),
    };
    let request = match body.and_then(|body| body.into_parts().0) {
        Ok(request) => request,
        Err(error) => return error.into_response(),
    };
    respond(
        service
            .submit(
                &caller.context,
                caller.credential.as_ref().map(DelegatedCredential::value),
                &BridgeInvokeGrantId(id),
                &key,
                request,
            )
            .await,
    )
}
