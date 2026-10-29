//! Narrow machine-to-machine classification; never returns resource identities.
use axum::{
    extract::{DefaultBodyLimit, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Extension, Json, Router,
};
use layer2_core::{sandbox::binding::BindingAccessService, SubjectId};
use serde::{Deserialize, Serialize};
use std::{sync::Arc, time::Duration};
use utoipa::ToSchema;

pub const WORKER_CLASSIFIER_PATH: &str = "/api/v1/cmaeg/worker-credentials/classify";

/// A composition boundary proves the exact bearer is a currently authorized
/// Gateway machine identity before installing this marker. Owner authentication
/// and ordinary management capabilities alone must never install it.
#[derive(Clone)]
pub struct VerifiedGatewayService {
    _subject: SubjectId,
}
impl VerifiedGatewayService {
    pub fn new(subject: SubjectId) -> Self {
        Self { _subject: subject }
    }
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkerCredentialDigest {
    /// Bare SHA-256 digest; never send the raw credential.
    #[schema(min_length = 64, max_length = 64, pattern = "^[0-9a-f]{64}$")]
    pub sha256: String,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkerCredentialClassification {
    KnownWorker,
    Unknown,
}

#[derive(Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkerCredentialClassificationView {
    pub classification: WorkerCredentialClassification,
}

pub fn worker_credential_router(access: Arc<BindingAccessService>) -> Router {
    Router::new()
        .route(WORKER_CLASSIFIER_PATH, post(classify))
        .layer(DefaultBodyLimit::max(1024))
        .layer(axum::middleware::from_fn(no_store))
        .with_state(access)
}

#[utoipa::path(post, path = "/api/v1/cmaeg/worker-credentials/classify", tag = "provider bindings",
    description = "Gateway machine service only: verified NyxID service-account access bearer with both current cma:provider:bindings and cma:gateway:records capabilities. Browser, owner, worker, delegated and resource credentials are refused. Unknown means a successful current lookup found no current or historical worker digest; failures never mean unknown.",
    request_body = WorkerCredentialDigest,
    responses((status = 200, body = WorkerCredentialClassificationView), (status = 400), (status = 401), (status = 403), (status = 503)))]
async fn classify(
    State(access): State<Arc<BindingAccessService>>,
    service: Option<Extension<VerifiedGatewayService>>,
    body: Result<Json<WorkerCredentialDigest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    if service.is_none() {
        return failure(
            StatusCode::UNAUTHORIZED,
            "worker_classifier_auth_required",
            "Gateway machine service authentication is required.",
        );
    }
    let digest = match body {
        Ok(Json(body))
            if body.sha256.len() == 64
                && body
                    .sha256
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)) =>
        {
            body.sha256
        }
        _ => {
            return failure(
                StatusCode::BAD_REQUEST,
                "worker_classifier_invalid_digest",
                "Supply exactly one lowercase 64-character SHA-256 digest.",
            )
        }
    };
    let classification = match tokio::time::timeout(
        Duration::from_secs(1),
        access.classify_worker_key_hash(&digest),
    )
    .await
    {
        Ok(Ok(true)) => WorkerCredentialClassification::KnownWorker,
        Ok(Ok(false)) => WorkerCredentialClassification::Unknown,
        _ => {
            return failure(
                StatusCode::SERVICE_UNAVAILABLE,
                "worker_classifier_unavailable",
                "Worker credential classification is temporarily unavailable.",
            )
        }
    };
    Json(WorkerCredentialClassificationView { classification }).into_response()
}

pub fn failure(status: StatusCode, code: &str, detail: &str) -> Response {
    let mut response = super::problem_response(
        status,
        super::problem_document(status, &code.replace('_', "-"), detail, code),
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    response
}

async fn no_store(request: axum::extract::Request, next: axum::middleware::Next) -> Response {
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    response
}

#[derive(utoipa::OpenApi)]
#[openapi(
    paths(classify),
    components(schemas(
        WorkerCredentialDigest,
        WorkerCredentialClassification,
        WorkerCredentialClassificationView
    ))
)]
struct WorkerCredentialApiDoc;
pub(crate) fn document() -> utoipa::openapi::OpenApi {
    <WorkerCredentialApiDoc as utoipa::OpenApi>::openapi()
}

#[cfg(test)]
pub(super) mod tests;
