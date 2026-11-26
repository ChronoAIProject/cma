//! Owner redemption and operator admission for an exact retained runtime transition.
use std::sync::Arc;

use async_trait::async_trait;
use axum::{
    extract::{rejection::JsonRejection, State},
    http::StatusCode,
    routing::{post, put},
    Extension, Json, Router,
};
use layer2_core::{
    resource_access::{CallerContext, CredentialSource, ResourceActor},
    Capability, SandboxId, SubjectId,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::runtime_canary::{CanaryPolicyRequest, CanaryPolicyView, CanaryRefusal};
use crate::AuthenticatedCaller;

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RetainedRuntimeProposal {
    #[schema(value_type = String)]
    pub sandbox_id: SandboxId,
    #[schema(value_type = String)]
    pub owner: SubjectId,
    pub sandbox_revision: u64,
    pub release_key: String,
    pub policy_revision: u64,
    pub journal_recovery: bool,
}

#[derive(Serialize, ToSchema)]
pub struct RetainedRuntimePreview {
    pub predecessor_image: String,
    pub predecessor_generation: u64,
    pub predecessor_journal_recovery: bool,
    pub target_image: String,
    pub target_journal_recovery: bool,
    pub plan_sha256: String,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RetainedRuntimeGrantRequest {
    pub proposal: RetainedRuntimeProposal,
    pub grant_id: String,
    pub plan_sha256: String,
    pub consent_sha256: String,
    pub custody_sha256: String,
    pub expires_at: String,
}

#[derive(Serialize, ToSchema)]
pub struct RetainedRuntimeGrantView {
    pub grant_id: String,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RetainedRuntimeAbort {
    #[schema(value_type = String)]
    pub owner: SubjectId,
    pub grant_id: String,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RetainedRuntimeRedemption {
    pub grant_id: String,
}

#[derive(Serialize, ToSchema)]
pub struct RetainedRuntimeReceipt {
    pub operation_id: String,
    pub successor_generation: u64,
}

#[async_trait]
pub trait RetainedRuntimeTransitions: Send + Sync {
    async fn configure(
        &self,
        request: CanaryPolicyRequest,
    ) -> Result<CanaryPolicyView, CanaryRefusal>;
    async fn preview(
        &self,
        request: RetainedRuntimeProposal,
    ) -> Result<RetainedRuntimePreview, CanaryRefusal>;
    async fn grant(
        &self,
        operator: &SubjectId,
        request: RetainedRuntimeGrantRequest,
    ) -> Result<RetainedRuntimeGrantView, CanaryRefusal>;
    async fn abort(
        &self,
        request: RetainedRuntimeAbort,
    ) -> Result<RetainedRuntimeGrantView, CanaryRefusal>;
    async fn redeem(
        &self,
        owner: &SubjectId,
        request: RetainedRuntimeRedemption,
    ) -> Result<RetainedRuntimeReceipt, CanaryRefusal>;
}

pub fn router(service: Arc<dyn RetainedRuntimeTransitions>) -> Router {
    Router::new()
        .route("/api/v1/admin/retained-runtime-policy", put(configure))
        .route("/api/v1/admin/retained-runtime-preview", post(preview))
        .route("/api/v1/admin/retained-runtime-grants", post(grant))
        .route("/api/v1/admin/retained-runtime-grants/abort", post(abort))
        .route("/api/v1/retained-runtime-redemptions", post(redeem))
        .with_state(service)
}

#[utoipa::path(put, path = "/api/v1/admin/retained-runtime-policy", operation_id = "configureRetainedRuntimePolicy", tag = "administration",
    request_body = CanaryPolicyRequest, responses(
        (status = 200, body = CanaryPolicyView), (status = 401), (status = 403), (status = 409), (status = 422), (status = 503)
    ))]
async fn configure(
    State(service): State<Arc<dyn RetainedRuntimeTransitions>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    body: Result<Json<CanaryPolicyRequest>, JsonRejection>,
) -> Result<Json<CanaryPolicyView>, CanaryRefusal> {
    caller
        .require(Capability::ConfigurePlatform)
        .map_err(|_| CanaryRefusal::Forbidden)?;
    service
        .configure(body.map_err(|_| CanaryRefusal::Invalid)?.0)
        .await
        .map(Json)
}

#[utoipa::path(post, path = "/api/v1/admin/retained-runtime-preview", operation_id = "previewRetainedRuntime", tag = "administration",
    request_body = RetainedRuntimeProposal, responses(
        (status = 200, body = RetainedRuntimePreview), (status = 401), (status = 403), (status = 409), (status = 422), (status = 503)
    ))]
async fn preview(
    State(service): State<Arc<dyn RetainedRuntimeTransitions>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    body: Result<Json<RetainedRuntimeProposal>, JsonRejection>,
) -> Result<Json<RetainedRuntimePreview>, CanaryRefusal> {
    caller
        .require(Capability::ConfigurePlatform)
        .map_err(|_| CanaryRefusal::Forbidden)?;
    service
        .preview(body.map_err(|_| CanaryRefusal::Invalid)?.0)
        .await
        .map(Json)
}

#[utoipa::path(post, path = "/api/v1/admin/retained-runtime-grants", operation_id = "grantRetainedRuntime", tag = "administration",
    request_body = RetainedRuntimeGrantRequest, responses(
        (status = 201, body = RetainedRuntimeGrantView), (status = 401), (status = 403), (status = 409), (status = 422), (status = 503)
    ))]
async fn grant(
    State(service): State<Arc<dyn RetainedRuntimeTransitions>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    body: Result<Json<RetainedRuntimeGrantRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<RetainedRuntimeGrantView>), CanaryRefusal> {
    caller
        .require(Capability::ConfigurePlatform)
        .map_err(|_| CanaryRefusal::Forbidden)?;
    let granted = service
        .grant(&caller.subject, body.map_err(|_| CanaryRefusal::Invalid)?.0)
        .await?;
    Ok((StatusCode::CREATED, Json(granted)))
}

#[utoipa::path(post, path = "/api/v1/admin/retained-runtime-grants/abort",
    operation_id = "abortRetainedRuntimeGrant", tag = "administration",
    request_body = RetainedRuntimeAbort, responses(
        (status = 200, body = RetainedRuntimeGrantView), (status = 401), (status = 403),
        (status = 409), (status = 422), (status = 503)
    ))]
async fn abort(
    State(service): State<Arc<dyn RetainedRuntimeTransitions>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    body: Result<Json<RetainedRuntimeAbort>, JsonRejection>,
) -> Result<Json<RetainedRuntimeGrantView>, CanaryRefusal> {
    caller
        .require(Capability::ConfigurePlatform)
        .map_err(|_| CanaryRefusal::Forbidden)?;
    service
        .abort(body.map_err(|_| CanaryRefusal::Invalid)?.0)
        .await
        .map(Json)
}

#[utoipa::path(post, path = "/api/v1/retained-runtime-redemptions", operation_id = "redeemRetainedRuntime", tag = "sandboxes",
    request_body = RetainedRuntimeRedemption, responses(
        (status = 202, body = RetainedRuntimeReceipt), (status = 401), (status = 403), (status = 409), (status = 422), (status = 503)
    ))]
async fn redeem(
    State(service): State<Arc<dyn RetainedRuntimeTransitions>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    context: Option<Extension<CallerContext>>,
    body: Result<Json<RetainedRuntimeRedemption>, JsonRejection>,
) -> Result<(StatusCode, Json<RetainedRuntimeReceipt>), CanaryRefusal> {
    caller
        .require(Capability::UseOwnSandboxes)
        .map_err(|_| CanaryRefusal::Forbidden)?;
    let Extension(context) = context.ok_or(CanaryRefusal::Forbidden)?;
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
        return Err(CanaryRefusal::Forbidden);
    }
    let receipt = service
        .redeem(&caller.subject, body.map_err(|_| CanaryRefusal::Invalid)?.0)
        .await?;
    Ok((StatusCode::ACCEPTED, Json(receipt)))
}

pub(super) fn document() -> utoipa::openapi::OpenApi {
    use utoipa::OpenApi as _;
    #[derive(utoipa::OpenApi)]
    #[openapi(paths(configure, preview, grant, abort, redeem))]
    struct RetainedRuntimeApi;
    RetainedRuntimeApi::openapi()
}
