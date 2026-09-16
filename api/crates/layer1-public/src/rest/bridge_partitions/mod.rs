//! Authorized read-only status for exact retained allocation generations.
use crate::{AuthenticatedResourceCaller, DelegatedCredential};
use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use layer2_core::bridge::{status::*, BridgeId};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
mod shapes;
pub(crate) mod usage;
use shapes::*;

pub fn bridge_partitions_router(service: Option<Arc<BridgePartitionService>>) -> Router {
    Router::new()
        .route("/api/v1/bridges/{id}/usage", get(usage::read))
        .route("/api/v1/bridges/{id}/partitions", get(list))
        .route("/api/v1/bridges/{id}/partitions/{allocation}", get(detail))
        .layer(axum::middleware::from_fn(private_headers))
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
fn respond<T: Serialize>(result: Result<T, PartitionStatusError>) -> Response {
    match result {
        Ok(value) => Json(value).into_response(),
        Err(error) => problem(error),
    }
}
fn problem(error: PartitionStatusError) -> Response {
    use PartitionStatusError::*;
    let (status, code, detail) = match error {
        Bridge(error) => return super::bridges::problem::BridgeProblem(error).into_response(),
        Invalid => (StatusCode::BAD_REQUEST, "invalid_bridge_partition_query", "Provide an unchanged cursor from this Bridge, a limit from 1 through 25, or an exact allocation ID."),
        NotFound => (StatusCode::NOT_FOUND, "bridge_partition_not_found", "The partition was not found in this Bridge."),
        Conflict => (StatusCode::CONFLICT, "bridge_partition_changed", "Partition state or access changed during the read. Refresh to observe current status."),
        Unavailable => (StatusCode::SERVICE_UNAVAILABLE, "bridge_partition_unavailable", "Current partition state or authority could not be checked."),
        Corrupt => (StatusCode::SERVICE_UNAVAILABLE, "bridge_partition_evidence_inconsistent", "Retained partition evidence could not be verified."),
    };
    super::problem_response(status, super::problem_document(status, code, detail, code))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ListParams {
    cursor: Option<String>,
    limit: Option<u32>,
}

#[utoipa::path(get,path="/api/v1/bridges/{id}/partitions",tag="bridges",params(("id"=String,Path),("cursor"=Option<String>,Query),("limit"=Option<u32>,Query)),responses((status=200,body=BridgePartitionPageShape),(status=400,body=super::ProblemDocument),(status=403,body=super::ProblemDocument),(status=404,body=super::ProblemDocument),(status=409,body=super::ProblemDocument),(status=503,body=super::ProblemDocument)))]
pub(crate) async fn list(
    State(service): State<Option<Arc<BridgePartitionService>>>,
    caller: AuthenticatedResourceCaller,
    Path(id): Path<String>,
    params: Result<Query<ListParams>, axum::extract::rejection::QueryRejection>,
) -> Response {
    let Some(service) = service else {
        return problem(PartitionStatusError::Unavailable);
    };
    let Ok(Query(params)) = params else {
        return problem(PartitionStatusError::Invalid);
    };
    respond(
        service
            .list(
                &caller.context,
                credential(&caller),
                &BridgeId(id),
                params.cursor.as_deref(),
                params.limit.unwrap_or(25),
            )
            .await,
    )
}

#[utoipa::path(get,path="/api/v1/bridges/{id}/partitions/{allocation}",tag="bridges",params(("id"=String,Path),("allocation"=String,Path)),responses((status=200,body=BridgePartitionDetailShape),(status=400,body=super::ProblemDocument),(status=403,body=super::ProblemDocument),(status=404,body=super::ProblemDocument),(status=409,body=super::ProblemDocument),(status=503,body=super::ProblemDocument)))]
pub(crate) async fn detail(
    State(service): State<Option<Arc<BridgePartitionService>>>,
    caller: AuthenticatedResourceCaller,
    Path((id, allocation)): Path<(String, String)>,
) -> Response {
    let Some(service) = service else {
        return problem(PartitionStatusError::Unavailable);
    };
    respond(
        service
            .detail(
                &caller.context,
                credential(&caller),
                &BridgeId(id),
                &allocation,
            )
            .await,
    )
}
