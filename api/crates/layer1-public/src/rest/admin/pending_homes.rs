//! Explicit, audited counter correction after receipt investigation.
use super::AdminState;
use crate::AuthenticatedCaller;
use axum::{
    extract::{rejection::JsonRejection, State},
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use layer2_core::{admission::PendingHomesReconciliation, Capability, CoreError};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::{OpenApi, ToSchema};

pub(super) fn routes() -> Router<Arc<AdminState>> {
    Router::new().route(
        "/api/v1/admin/admission/pending-homes/reconcile",
        post(reconcile),
    )
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct ReconcilePendingHomesRequest {
    /// Nonblank text, at most 512 UTF-8 bytes, with no control characters.
    reason: String,
    /// Counter returned by the operator's most recent observation.
    observed_pending: u64,
    /// Defaults to true. A dry run performs no writes or audit inserts.
    #[serde(default = "default_dry_run")]
    #[schema(default = true)]
    dry_run: bool,
}
fn default_dry_run() -> bool {
    true
}

#[derive(Serialize, ToSchema)]
struct PendingHomesReconciliationView {
    previous_pending_launch_homes: u64,
    expected_pending_launch_homes: u64,
    pending_launch_homes: u64,
    dry_run: bool,
    corrected: bool,
}
impl From<PendingHomesReconciliation> for PendingHomesReconciliationView {
    fn from(value: PendingHomesReconciliation) -> Self {
        Self {
            previous_pending_launch_homes: value.previous_pending_launch_homes,
            expected_pending_launch_homes: value.expected_pending_launch_homes,
            pending_launch_homes: value.pending_launch_homes,
            dry_run: value.dry_run,
            corrected: value.corrected,
        }
    }
}

#[utoipa::path(post, path = "/api/v1/admin/admission/pending-homes/reconcile", tag = "administration",
    request_body = ReconcilePendingHomesRequest, responses(
        (status = 200, body = PendingHomesReconciliationView),
        (status = 403, body = crate::rest::ProblemDocument),
        (status = 409, body = crate::rest::ProblemDocument),
        (status = 422, body = crate::rest::ProblemDocument),
        (status = 503, body = crate::rest::ProblemDocument)))]
async fn reconcile(
    State(state): State<Arc<AdminState>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    body: Result<Json<ReconcilePendingHomesRequest>, JsonRejection>,
) -> Result<Json<PendingHomesReconciliationView>, Failure> {
    caller.require(Capability::ConfigurePlatform)?;
    let service = state
        .admission_ceilings
        .as_ref()
        .ok_or_else(|| CoreError::RuntimeUnavailable("admission ceilings unavailable".into()))?;
    let Json(body) =
        body.map_err(|_| CoreError::InvalidRequest("invalid pending home request".into()))?;
    let result = service
        .reconcile_pending_homes(&caller, body.observed_pending, &body.reason, body.dry_run)
        .await?;
    Ok(Json(result.into()))
}
struct Failure(CoreError);
impl From<CoreError> for Failure {
    fn from(value: CoreError) -> Self {
        Self(value)
    }
}
impl IntoResponse for Failure {
    fn into_response(self) -> Response {
        let (status, slug, detail, code) = match self.0 {
            CoreError::Forbidden => return super::NotAnOperator.into_response(),
            CoreError::AdmissionCeilingsWritesUnavailable => (StatusCode::SERVICE_UNAVAILABLE,
                "admission-ceilings-writes-unavailable", "Saving admission ceilings is not available yet.", "admission_ceilings_writes_unavailable"),
            CoreError::InvalidRequest(_) => (StatusCode::UNPROCESSABLE_ENTITY, "invalid-pending-homes-reconciliation",
                "Supply a bounded reason and a nonnegative observed pending count.", "invalid_request"),
            CoreError::InvalidState(_) => (StatusCode::CONFLICT, "pending-homes-conflict",
                "The pending home counter changed. Read the current state before issuing a new command.", "pending_homes_conflict"),
            _ => (StatusCode::SERVICE_UNAVAILABLE, "pending-homes-unavailable",
                "Pending homes could not be reconciled. Read the current state before retrying an uncertain command.", "pending_homes_unavailable"),
        };
        let mut response = crate::rest::problem_response(
            status,
            crate::rest::problem_document(status, slug, detail, code),
        );
        if status == StatusCode::SERVICE_UNAVAILABLE {
            response
                .headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from_static("60"));
        }
        response
    }
}
#[derive(OpenApi)]
#[openapi(
    paths(reconcile),
    components(schemas(ReconcilePendingHomesRequest, PendingHomesReconciliationView))
)]
struct PendingHomesApi;
pub(crate) fn document() -> utoipa::openapi::OpenApi {
    PendingHomesApi::openapi()
}
