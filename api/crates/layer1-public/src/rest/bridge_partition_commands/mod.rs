use super::{
    idempotency::{IdempotencyKey, RequestBody},
    BodyProblem,
};
use crate::{AuthenticatedResourceCaller, DelegatedCredential};
use axum::{
    extract::{Path, State},
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use layer2_core::bridge::{partition_commands::*, BridgeId};
use std::sync::Arc;
mod problem;
mod shapes;
use shapes::*;

pub fn bridge_partition_commands_router(
    service: Option<Arc<BridgePartitionCommandService>>,
) -> Router {
    Router::new()
        .route(
            "/api/v1/bridges/{id}/partitions/{allocation}/reset",
            post(reset),
        )
        .route(
            "/api/v1/bridges/{id}/partitions/{allocation}/detach",
            post(detach),
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
#[utoipa::path(post,path="/api/v1/bridges/{id}/partitions/{allocation}/reset",tag="bridges",params(("id"=String,Path),("allocation"=String,Path),("Idempotency-Key"=String,Header)),request_body=PartitionCommandBody,responses((status=202,body=PartitionCommandReceiptShape),(status=400,body=super::ProblemDocument),(status=403,body=super::ProblemDocument),(status=404,body=super::ProblemDocument),(status=409,body=super::ProblemDocument),(status=415,body=super::ProblemDocument),(status=422,body=super::ProblemDocument),(status=503,body=super::ProblemDocument)))]
pub(crate) async fn reset(
    State(service): State<Option<Arc<BridgePartitionCommandService>>>,
    caller: AuthenticatedResourceCaller,
    Path((id, allocation)): Path<(String, String)>,
    key: IdempotencyKey,
    input: Result<RequestBody<PartitionCommandBody>, BodyProblem>,
) -> Response {
    command(
        service,
        caller,
        id,
        allocation,
        key,
        input,
        PartitionCommandAction::Reset,
    )
    .await
}
#[utoipa::path(post,path="/api/v1/bridges/{id}/partitions/{allocation}/detach",tag="bridges",params(("id"=String,Path),("allocation"=String,Path),("Idempotency-Key"=String,Header)),request_body=PartitionCommandBody,responses((status=202,body=PartitionCommandReceiptShape),(status=400,body=super::ProblemDocument),(status=403,body=super::ProblemDocument),(status=404,body=super::ProblemDocument),(status=409,body=super::ProblemDocument),(status=415,body=super::ProblemDocument),(status=422,body=super::ProblemDocument),(status=503,body=super::ProblemDocument)))]
pub(crate) async fn detach(
    State(service): State<Option<Arc<BridgePartitionCommandService>>>,
    caller: AuthenticatedResourceCaller,
    Path((id, allocation)): Path<(String, String)>,
    key: IdempotencyKey,
    input: Result<RequestBody<PartitionCommandBody>, BodyProblem>,
) -> Response {
    command(
        service,
        caller,
        id,
        allocation,
        key,
        input,
        PartitionCommandAction::Detach,
    )
    .await
}
async fn command(
    service: Option<Arc<BridgePartitionCommandService>>,
    caller: AuthenticatedResourceCaller,
    id: String,
    allocation_id: String,
    key: IdempotencyKey,
    input: Result<RequestBody<PartitionCommandBody>, BodyProblem>,
    action: PartitionCommandAction,
) -> Response {
    let Some(service) = service else {
        return problem::CommandProblem(PartitionCommandError::Unavailable).into_response();
    };
    let key = match key.into_required_value() {
        Ok(key) => key,
        Err(error) => return error.into_response(),
    };
    let body = match input.and_then(|body| body.into_parts().0) {
        Ok(body) => body,
        Err(error) => return error.into_response(),
    };
    match service
        .command(
            &caller.context,
            caller.credential.as_ref().map(DelegatedCredential::value),
            &BridgeId(id),
            &key,
            PartitionCommandRequest {
                allocation_id,
                action,
                expected_bridge_revision: body.expected_bridge_revision,
                expected_revision: body.expected_revision,
            },
        )
        .await
    {
        Ok(receipt) => (StatusCode::ACCEPTED, Json(receipt)).into_response(),
        Err(error) => problem::CommandProblem(error).into_response(),
    }
}
