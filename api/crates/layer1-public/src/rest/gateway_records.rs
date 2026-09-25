//! REST projection of opaque event-gateway channel records.

use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, put},
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use layer2_core::{
    CoreError, GatewayRecord, GatewayRecordHead, GatewayRecordService, SubjectId,
    MAX_GATEWAY_RECORD_BLOB_BYTES,
};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::{rest::problem_document, AuthenticatedCaller};

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PutGatewayRecordRequest {
    pub owner: String,
    pub blob: String,
    pub expected_version: Option<u64>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct PutGatewayRecordView {
    pub record_id: String,
    pub version: u64,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct GatewayRecordView {
    pub record_id: String,
    pub owner: String,
    pub blob: String,
    pub version: u64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct GatewayRecordHeadView {
    pub record_id: String,
    pub owner: String,
    pub version: u64,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct GatewayRecordPageView {
    pub items: Vec<GatewayRecordHeadView>,
    pub next_after: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, IntoParams)]
pub struct GatewayRecordListQuery {
    pub owner: Option<String>,
    pub after: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Default, Deserialize, IntoParams)]
pub struct GatewayRecordDeleteQuery {
    pub expected_version: Option<u64>,
}

pub fn gateway_record_router(service: std::sync::Arc<GatewayRecordService>) -> Router {
    Router::new()
        .route("/api/v1/gateway/channel-records", get(list))
        .route(
            "/api/v1/gateway/channel-records/{record_id}",
            put(put_record).get(get_record).delete(delete_record),
        )
        .with_state(service)
        .layer(axum::middleware::from_fn(add_no_store))
}

async fn add_no_store(request: axum::extract::Request, next: axum::middleware::Next) -> Response {
    no_store(next.run(request).await)
}

#[utoipa::path(
    put,
    path = "/api/v1/gateway/channel-records/{record_id}",
    tag = "event gateway",
    params(("record_id" = String, Path)),
    request_body = PutGatewayRecordRequest,
    responses((status = 200, body = PutGatewayRecordView), (status = 400, body = crate::rest::ProblemDocument), (status = 401, body = crate::rest::ProblemDocument), (status = 403, body = crate::rest::ProblemDocument), (status = 409, body = crate::rest::ProblemDocument), (status = 413, body = crate::rest::ProblemDocument), (status = 503, body = crate::rest::ProblemDocument))
)]
async fn put_record(
    State(service): State<std::sync::Arc<GatewayRecordService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(record_id): Path<String>,
    body: Result<Json<PutGatewayRecordRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let body = match body {
        Ok(Json(body)) => body,
        Err(error) => return no_store(super::BodyProblem::from(error).into_response()),
    };
    let blob = match STANDARD.decode(body.blob.as_bytes()) {
        Ok(blob) if blob.is_empty() => {
            return no_store(problem(
                StatusCode::BAD_REQUEST,
                "invalid_gateway_record",
                "blob must contain at least one decoded byte",
                "invalid_gateway_record",
            ))
        }
        Ok(blob) if blob.len() <= MAX_GATEWAY_RECORD_BLOB_BYTES => blob,
        Ok(_) => return too_large_or_empty(),
        Err(error) => {
            return no_store(problem(
                StatusCode::BAD_REQUEST,
                "invalid_gateway_record",
                &format!("blob is not valid base64: {error}"),
                "invalid_gateway_record",
            ))
        }
    };
    if body.owner.is_empty() {
        return no_store(problem(
            StatusCode::BAD_REQUEST,
            "invalid_gateway_record",
            "owner must not be empty",
            "invalid_gateway_record",
        ));
    }
    match service
        .put(
            &caller,
            &record_id,
            &SubjectId::new(body.owner),
            blob,
            body.expected_version,
        )
        .await
    {
        Ok(record) => no_store(Json(PutGatewayRecordView::from(record)).into_response()),
        Err(error) => gateway_error(error),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/gateway/channel-records/{record_id}",
    tag = "event gateway",
    params(("record_id" = String, Path)),
    responses((status = 200, body = GatewayRecordView), (status = 400, body = crate::rest::ProblemDocument), (status = 401, body = crate::rest::ProblemDocument), (status = 403, body = crate::rest::ProblemDocument), (status = 404, body = crate::rest::ProblemDocument), (status = 503, body = crate::rest::ProblemDocument))
)]
async fn get_record(
    State(service): State<std::sync::Arc<GatewayRecordService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(record_id): Path<String>,
) -> Response {
    match service.get(&caller, &record_id).await {
        Ok(Some(record)) => no_store(Json(GatewayRecordView::from(record)).into_response()),
        Ok(None) => not_found(),
        Err(error) => gateway_error(error),
    }
}

#[utoipa::path(
    delete,
    path = "/api/v1/gateway/channel-records/{record_id}",
    tag = "event gateway",
    params(("record_id" = String, Path), GatewayRecordDeleteQuery),
    responses((status = 204), (status = 400, body = crate::rest::ProblemDocument), (status = 401, body = crate::rest::ProblemDocument), (status = 403, body = crate::rest::ProblemDocument), (status = 404, body = crate::rest::ProblemDocument), (status = 409, body = crate::rest::ProblemDocument), (status = 503, body = crate::rest::ProblemDocument))
)]
async fn delete_record(
    State(service): State<std::sync::Arc<GatewayRecordService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(record_id): Path<String>,
    query: Result<Query<GatewayRecordDeleteQuery>, axum::extract::rejection::QueryRejection>,
) -> Response {
    let query = match query {
        Ok(Query(query)) => query,
        Err(error) => {
            return no_store(problem(
                StatusCode::BAD_REQUEST,
                "invalid_query",
                &error.body_text(),
                "invalid_query",
            ))
        }
    };
    match service
        .delete(&caller, &record_id, query.expected_version)
        .await
    {
        Ok(true) => no_store(StatusCode::NO_CONTENT.into_response()),
        Ok(false) => not_found(),
        Err(error) => gateway_error(error),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/gateway/channel-records",
    tag = "event gateway",
    params(GatewayRecordListQuery),
    responses((status = 200, body = GatewayRecordPageView), (status = 400, body = crate::rest::ProblemDocument), (status = 401, body = crate::rest::ProblemDocument), (status = 403, body = crate::rest::ProblemDocument), (status = 503, body = crate::rest::ProblemDocument))
)]
async fn list(
    State(service): State<std::sync::Arc<GatewayRecordService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    query: Result<Query<GatewayRecordListQuery>, axum::extract::rejection::QueryRejection>,
) -> Response {
    let query = match query {
        Ok(Query(query)) => query,
        Err(error) => {
            return no_store(problem(
                StatusCode::BAD_REQUEST,
                "invalid_query",
                &error.body_text(),
                "invalid_query",
            ))
        }
    };
    let limit = query.limit.unwrap_or(100);
    let owner = query.owner.as_deref().map(SubjectId::new);
    match service
        .list(&caller, owner.as_ref(), query.after.as_deref(), limit)
        .await
    {
        Ok(items) => {
            let next_after = (items.len() == limit)
                .then(|| items.last().map(|item| item.record_id.clone()))
                .flatten();
            no_store(
                Json(GatewayRecordPageView {
                    items: items.into_iter().map(Into::into).collect(),
                    next_after,
                })
                .into_response(),
            )
        }
        Err(error) => gateway_error(error),
    }
}

impl From<GatewayRecord> for PutGatewayRecordView {
    fn from(record: GatewayRecord) -> Self {
        Self {
            record_id: record.record_id,
            version: record.version,
            updated_at: record.updated_at,
        }
    }
}

impl From<GatewayRecord> for GatewayRecordView {
    fn from(record: GatewayRecord) -> Self {
        Self {
            record_id: record.record_id,
            owner: record.owner.as_str().into(),
            blob: STANDARD.encode(record.blob),
            version: record.version,
            created_at: record.created_at,
            updated_at: record.updated_at,
        }
    }
}

impl From<GatewayRecordHead> for GatewayRecordHeadView {
    fn from(record: GatewayRecordHead) -> Self {
        Self {
            record_id: record.record_id,
            owner: record.owner.as_str().into(),
            version: record.version,
            updated_at: record.updated_at,
        }
    }
}

pub(super) fn gateway_error(error: CoreError) -> Response {
    match error {
        CoreError::Forbidden => no_store(problem(
            StatusCode::FORBIDDEN,
            "forbidden",
            "Current authority does not permit gateway record management.",
            "forbidden",
        )),
        CoreError::InvalidState(detail) => no_store(problem(
            StatusCode::CONFLICT,
            "conflict",
            &detail,
            "conflict",
        )),
        CoreError::InvalidRequest(detail) => {
            if detail.contains("blob must be between") {
                too_large_or_empty()
            } else {
                no_store(problem(
                    StatusCode::BAD_REQUEST,
                    "invalid_gateway_record",
                    &detail,
                    "invalid_gateway_record",
                ))
            }
        }
        CoreError::RuntimeUnavailable(detail) => no_store(problem(
            StatusCode::SERVICE_UNAVAILABLE,
            "store_failed",
            &detail,
            "store_failed",
        )),
        other => no_store(problem(
            StatusCode::SERVICE_UNAVAILABLE,
            "store_failed",
            &other.to_string(),
            "store_failed",
        )),
    }
}

fn too_large_or_empty() -> Response {
    no_store(problem(
        StatusCode::PAYLOAD_TOO_LARGE,
        "gateway_record_too_large",
        "The gateway record blob must be between 1 and 65536 decoded bytes.",
        "gateway_record_too_large",
    ))
}

fn not_found() -> Response {
    no_store(problem(
        StatusCode::NOT_FOUND,
        "gateway_record_not_found",
        "The gateway channel record does not exist.",
        "gateway_record_not_found",
    ))
}

fn problem(status: StatusCode, slug: &str, detail: &str, code: &str) -> Response {
    super::problem_response(status, problem_document(status, slug, detail, code))
}

fn no_store(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}
