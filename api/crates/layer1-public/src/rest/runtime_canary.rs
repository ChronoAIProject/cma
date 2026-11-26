//! Protected operator consent and owner redemption for exact runtime canaries.

use std::sync::Arc;

use async_trait::async_trait;
use axum::{
    extract::{rejection::JsonRejection, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{post, put},
    Extension, Json, Router,
};
use layer2_core::{
    resource_access::{CallerContext, CredentialSource, ResourceActor},
    Capability, SubjectId,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::{
    idempotency::IdempotencyKey,
    problems::{problem_document, problem_response},
    AgentLifecycleCreateReceipt,
};
use crate::AuthenticatedCaller;

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CanaryPolicyRequest {
    pub expected_revision: i64,
    pub open: bool,
}

#[derive(Serialize, ToSchema)]
pub struct CanaryPolicyView {
    pub revision: i64,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CanaryGrantRequest {
    #[schema(value_type = String)]
    pub owner: SubjectId,
    pub release_key: String,
    pub expected_revision: i64,
    pub policy_revision: i64,
    pub consent_sha256: String,
    pub expires_at: String,
}

#[derive(Serialize, ToSchema)]
pub struct CanaryGrantView {
    pub grant_id: String,
    pub revision: i64,
    pub policy_revision: i64,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CanaryRevokeRequest {
    #[schema(value_type = String)]
    pub owner: SubjectId,
    pub release_key: String,
    pub grant_id: String,
    pub expected_revision: i64,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CanaryRedeemRequest {
    pub grant_id: String,
    pub release_key: String,
    pub policy_revision: i64,
    pub name: String,
    pub initial_message: String,
}

#[derive(Debug)]
pub enum CanaryRefusal {
    Forbidden,
    Invalid,
    Closed,
    Conflict,
    Unavailable,
}

impl IntoResponse for CanaryRefusal {
    fn into_response(self) -> Response {
        let (status, slug, detail, code) = match self {
            Self::Forbidden => (StatusCode::FORBIDDEN, "runtime-canary-forbidden", "Runtime canary authority is unavailable to this caller.", "runtime_canary_forbidden"),
            Self::Invalid => (StatusCode::UNPROCESSABLE_ENTITY, "runtime-canary-invalid", "The runtime canary request is invalid.", "runtime_canary_invalid"),
            Self::Closed => (StatusCode::CONFLICT, "runtime-canary-closed", "Fresh runtime canary admission is closed.", "runtime_canary_closed"),
            Self::Conflict => (StatusCode::CONFLICT, "runtime-canary-conflict", "The request differs from the current runtime canary authority.", "runtime_canary_conflict"),
            Self::Unavailable => (StatusCode::SERVICE_UNAVAILABLE, "runtime-canary-unavailable", "Runtime canary persistence is unresolved. Retry this request with its original key.", "runtime_canary_unavailable"),
        };
        problem_response(status, problem_document(status, slug, detail, code))
    }
}

#[async_trait]
pub trait RuntimeCanaryService: Send + Sync {
    async fn configure(
        &self,
        request: CanaryPolicyRequest,
    ) -> Result<CanaryPolicyView, CanaryRefusal>;
    async fn grant(&self, request: CanaryGrantRequest) -> Result<CanaryGrantView, CanaryRefusal>;
    async fn revoke(&self, request: CanaryRevokeRequest) -> Result<CanaryGrantView, CanaryRefusal>;
    async fn redeem(
        &self,
        context: &CallerContext,
        key: &str,
        request: CanaryRedeemRequest,
    ) -> Result<AgentLifecycleCreateReceipt, CanaryRefusal>;
}

pub fn runtime_canary_router(service: Arc<dyn RuntimeCanaryService>) -> Router {
    Router::new()
        .route("/api/v1/admin/runtime-canary-policy", put(configure))
        .route("/api/v1/admin/runtime-canary-grants", post(grant))
        .route("/api/v1/admin/runtime-canary-grants/revoke", post(revoke))
        .route("/api/v1/runtime-canary-redemptions", post(redeem))
        .with_state(service)
}

#[utoipa::path(
    put, path = "/api/v1/admin/runtime-canary-policy", tag = "administration", request_body = CanaryPolicyRequest,
    description = "Set canary admission using the latest policy revision. Revision zero creates the first policy.",
    responses(
        (status = 200, body = CanaryPolicyView),
        (status = 403, body = crate::rest::ProblemDocument, content_type = "application/problem+json"),
        (status = 409, body = crate::rest::ProblemDocument, content_type = "application/problem+json"),
        (status = 422, body = crate::rest::ProblemDocument, content_type = "application/problem+json"),
        (status = 503, body = crate::rest::ProblemDocument, content_type = "application/problem+json"),
        (status = 401, body = crate::rest::ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub(crate) async fn configure(
    State(service): State<Arc<dyn RuntimeCanaryService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    body: Result<Json<CanaryPolicyRequest>, JsonRejection>,
) -> Result<Json<CanaryPolicyView>, CanaryRefusal> {
    caller
        .require(Capability::ConfigurePlatform)
        .map_err(|_| CanaryRefusal::Forbidden)?;
    let Json(request) = body.map_err(|_| CanaryRefusal::Invalid)?;
    service.configure(request).await.map(Json)
}

#[utoipa::path(
    post, path = "/api/v1/admin/runtime-canary-grants", tag = "administration", request_body = CanaryGrantRequest,
    description = "Issue one time-limited grant for the owner and exact catalog release. Repeat the issuance body to recover its original receipt while the slot is retained. The consent hash records the owner's approval.",
    responses(
        (status = 201, body = CanaryGrantView),
        (status = 403, body = crate::rest::ProblemDocument, content_type = "application/problem+json"),
        (status = 409, body = crate::rest::ProblemDocument, content_type = "application/problem+json"),
        (status = 422, body = crate::rest::ProblemDocument, content_type = "application/problem+json"),
        (status = 503, body = crate::rest::ProblemDocument, content_type = "application/problem+json"),
        (status = 401, body = crate::rest::ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub(crate) async fn grant(
    State(service): State<Arc<dyn RuntimeCanaryService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    body: Result<Json<CanaryGrantRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<CanaryGrantView>), CanaryRefusal> {
    caller
        .require(Capability::ConfigurePlatform)
        .map_err(|_| CanaryRefusal::Forbidden)?;
    let Json(request) = body.map_err(|_| CanaryRefusal::Invalid)?;
    let grant = service.grant(request).await?;
    Ok((StatusCode::CREATED, Json(grant)))
}

#[utoipa::path(
    post, path = "/api/v1/admin/runtime-canary-grants/revoke", tag = "administration", request_body = CanaryRevokeRequest,
    description = "Revoke an unconsumed stored grant at the expected revision, including after a catalog change. An accepted redemption keeps running.",
    responses(
        (status = 200, body = CanaryGrantView),
        (status = 403, body = crate::rest::ProblemDocument, content_type = "application/problem+json"),
        (status = 409, body = crate::rest::ProblemDocument, content_type = "application/problem+json"),
        (status = 422, body = crate::rest::ProblemDocument, content_type = "application/problem+json"),
        (status = 503, body = crate::rest::ProblemDocument, content_type = "application/problem+json"),
        (status = 401, body = crate::rest::ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub(crate) async fn revoke(
    State(service): State<Arc<dyn RuntimeCanaryService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    body: Result<Json<CanaryRevokeRequest>, JsonRejection>,
) -> Result<Json<CanaryGrantView>, CanaryRefusal> {
    caller
        .require(Capability::ConfigurePlatform)
        .map_err(|_| CanaryRefusal::Forbidden)?;
    let Json(request) = body.map_err(|_| CanaryRefusal::Invalid)?;
    service.revoke(request).await.map(Json)
}

#[utoipa::path(
    post, path = "/api/v1/runtime-canary-redemptions", tag = "agents", request_body = CanaryRedeemRequest,
    description = "Redeem a grant for the authenticated owner. On an uncertain response, retry the same body and key to recover the original operation.",
    params(("Idempotency-Key" = String, Header, description = "Replay key for this owner's consent")),
    responses(
        (status = 400, body = crate::rest::ProblemDocument, content_type = "application/problem+json"),
        (status = 202, body = AgentLifecycleCreateReceipt),
        (status = 403, body = crate::rest::ProblemDocument, content_type = "application/problem+json"),
        (status = 409, body = crate::rest::ProblemDocument, content_type = "application/problem+json"),
        (status = 422, body = crate::rest::ProblemDocument, content_type = "application/problem+json"),
        (status = 503, body = crate::rest::ProblemDocument, content_type = "application/problem+json"),
        (status = 401, body = crate::rest::ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub(super) async fn redeem(
    State(service): State<Arc<dyn RuntimeCanaryService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    context: Option<Extension<CallerContext>>,
    key: IdempotencyKey,
    body: Result<Json<CanaryRedeemRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<AgentLifecycleCreateReceipt>), Response> {
    caller
        .require(Capability::UseOwnSandboxes)
        .map_err(|_| CanaryRefusal::Forbidden.into_response())?;
    caller
        .require(Capability::CreateSandbox)
        .map_err(|_| CanaryRefusal::Forbidden.into_response())?;
    let Extension(context) = context.ok_or_else(|| CanaryRefusal::Forbidden.into_response())?;
    if context.caller != caller
        || !context.validate_principal()
        || !matches!(
            context.credential_source,
            CredentialSource::Retained | CredentialSource::Independent { .. }
        )
        || !matches!(
            context.actor,
            ResourceActor::Human { .. }
                | ResourceActor::Api {
                    delegation_id: None,
                    ..
                }
        )
    {
        return Err(CanaryRefusal::Forbidden.into_response());
    }
    let key = key
        .into_required_value()
        .map_err(IntoResponse::into_response)?;
    let Json(request) = body.map_err(|_| CanaryRefusal::Invalid.into_response())?;
    let receipt = service
        .redeem(&context, &key, request)
        .await
        .map_err(IntoResponse::into_response)?;
    Ok((StatusCode::ACCEPTED, Json(receipt)))
}

pub(super) fn document() -> utoipa::openapi::OpenApi {
    use utoipa::OpenApi as _;

    #[derive(utoipa::OpenApi)]
    #[openapi(paths(configure, grant, revoke, redeem))]
    struct RuntimeCanaryApi;

    RuntimeCanaryApi::openapi()
}
