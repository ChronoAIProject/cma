use super::{
    idempotency::{IdempotencyKey, RequestBody},
    BodyProblem,
};
use crate::{AuthenticatedResourceCaller, DelegatedCredential};
use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderValue},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use layer2_core::bridge::{outbox::management::*, BridgeId};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
mod problem;
mod shapes;
use shapes::*;

pub fn bridge_outbox_router(service: Option<Arc<BridgeOutboxService>>) -> Router {
    Router::new()
        .route("/api/v1/bridges/{id}/deliveries", get(list))
        .route("/api/v1/bridges/{id}/deliveries/{item}", get(detail))
        .route("/api/v1/bridges/{id}/delivery-recovery", post(recover))
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
fn respond<T: Serialize>(result: Result<T, OutboxManagementError>) -> Response {
    match result {
        Ok(value) => Json(value).into_response(),
        Err(error) => problem::OutboxProblem(error).into_response(),
    }
}
fn unavailable() -> Response {
    problem::OutboxProblem(layer2_core::bridge::outbox::OutboxError::Unavailable.into())
        .into_response()
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ListParams {
    cursor: Option<String>,
    limit: Option<u32>,
}

#[utoipa::path(get,path="/api/v1/bridges/{id}/deliveries",tag="bridges",params(("id"=String,Path),("cursor"=Option<String>,Query),("limit"=Option<u32>,Query)),responses((status=200,body=OutboxPageShape),(status=400,body=super::ProblemDocument),(status=403,body=super::ProblemDocument),(status=404,body=super::ProblemDocument),(status=409,body=super::ProblemDocument),(status=503,body=super::ProblemDocument)))]
pub(crate) async fn list(
    State(service): State<Option<Arc<BridgeOutboxService>>>,
    caller: AuthenticatedResourceCaller,
    Path(id): Path<String>,
    params: Result<Query<ListParams>, axum::extract::rejection::QueryRejection>,
) -> Response {
    let Some(service) = service else {
        return unavailable();
    };
    let Ok(Query(params)) = params else {
        return problem::OutboxProblem(OutboxManagementError::Invalid).into_response();
    };
    respond(
        service
            .list(
                &caller.context,
                credential(&caller),
                &BridgeId(id),
                params.cursor.as_deref(),
                params.limit.unwrap_or(50),
            )
            .await,
    )
}
#[utoipa::path(get,path="/api/v1/bridges/{id}/deliveries/{item}",tag="bridges",params(("id"=String,Path),("item"=String,Path)),responses((status=200,body=OutboxItemShape),(status=400,body=super::ProblemDocument),(status=403,body=super::ProblemDocument),(status=404,body=super::ProblemDocument),(status=409,body=super::ProblemDocument),(status=503,body=super::ProblemDocument)))]
pub(crate) async fn detail(
    State(service): State<Option<Arc<BridgeOutboxService>>>,
    caller: AuthenticatedResourceCaller,
    Path((id, item)): Path<(String, String)>,
) -> Response {
    let Some(service) = service else {
        return unavailable();
    };
    respond(
        service
            .detail(&caller.context, credential(&caller), &BridgeId(id), &item)
            .await,
    )
}
#[utoipa::path(post,path="/api/v1/bridges/{id}/delivery-recovery",tag="bridges",params(("id"=String,Path),("Idempotency-Key"=String,Header)),request_body=RecoveryRequest,responses((status=200,body=OutboxRecoveryShape),(status=400,body=super::ProblemDocument),(status=403,body=super::ProblemDocument),(status=404,body=super::ProblemDocument),(status=409,body=super::ProblemDocument),(status=415,body=super::ProblemDocument),(status=422,body=super::ProblemDocument),(status=503,body=super::ProblemDocument)))]
pub(crate) async fn recover(
    State(service): State<Option<Arc<BridgeOutboxService>>>,
    caller: AuthenticatedResourceCaller,
    Path(id): Path<String>,
    key: IdempotencyKey,
    input: Result<RequestBody<RecoveryRequest>, BodyProblem>,
) -> Response {
    let Some(service) = service else {
        return unavailable();
    };
    let key = match key.into_required_value() {
        Ok(key) => key,
        Err(error) => return error.into_response(),
    };
    let input = match input.and_then(|body| body.into_parts().0) {
        Ok(input) => input,
        Err(error) => return error.into_response(),
    };
    respond(
        service
            .command(
                &caller.context,
                credential(&caller),
                &BridgeId(id),
                &key,
                OutboxRecoveryRequest {
                    item_id: input.item_id,
                    expected_revision: input.expected_revision,
                    expected_bridge_revision: input.expected_bridge_revision,
                    action: input.action,
                },
            )
            .await,
    )
}
