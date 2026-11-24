//! Protected operator consent and owner redemption for exact runtime canaries.

use std::sync::Arc;

use async_trait::async_trait;
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{post, put},
    Extension, Json, Router,
};
use layer2_core::{
    resource_access::{CallerContext, ResourceActor},
    Capability, SubjectId,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::{
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

async fn configure(
    State(service): State<Arc<dyn RuntimeCanaryService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Json(request): Json<CanaryPolicyRequest>,
) -> Result<Json<CanaryPolicyView>, CanaryRefusal> {
    caller
        .require(Capability::ConfigurePlatform)
        .map_err(|_| CanaryRefusal::Forbidden)?;
    service.configure(request).await.map(Json)
}

async fn grant(
    State(service): State<Arc<dyn RuntimeCanaryService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Json(request): Json<CanaryGrantRequest>,
) -> Result<(StatusCode, Json<CanaryGrantView>), CanaryRefusal> {
    caller
        .require(Capability::ConfigurePlatform)
        .map_err(|_| CanaryRefusal::Forbidden)?;
    let grant = service.grant(request).await?;
    Ok((StatusCode::CREATED, Json(grant)))
}

async fn revoke(
    State(service): State<Arc<dyn RuntimeCanaryService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Json(request): Json<CanaryRevokeRequest>,
) -> Result<Json<CanaryGrantView>, CanaryRefusal> {
    caller
        .require(Capability::ConfigurePlatform)
        .map_err(|_| CanaryRefusal::Forbidden)?;
    service.revoke(request).await.map(Json)
}

async fn redeem(
    State(service): State<Arc<dyn RuntimeCanaryService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Extension(context): Extension<CallerContext>,
    headers: HeaderMap,
    Json(request): Json<CanaryRedeemRequest>,
) -> Result<(StatusCode, Json<AgentLifecycleCreateReceipt>), CanaryRefusal> {
    caller
        .require(Capability::UseOwnSandboxes)
        .map_err(|_| CanaryRefusal::Forbidden)?;
    caller
        .require(Capability::CreateSandbox)
        .map_err(|_| CanaryRefusal::Forbidden)?;
    if context.caller.subject != caller.subject
        || !context.validate_principal()
        || !matches!(
            context.actor,
            ResourceActor::Human { .. }
                | ResourceActor::Api {
                    delegation_id: None,
                    ..
                }
        )
    {
        return Err(CanaryRefusal::Forbidden);
    }
    let key = headers
        .get("Idempotency-Key")
        .and_then(|header| header.to_str().ok())
        .filter(|key| layer2_core::operation::valid_idempotency_key(key.as_bytes()))
        .ok_or(CanaryRefusal::Invalid)?;
    let receipt = service.redeem(&context, key, request).await?;
    Ok((StatusCode::ACCEPTED, Json(receipt)))
}
