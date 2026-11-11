//! Read-only owner/operator maintenance assessment. Registration is default-off.

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use layer2_core::sandbox::ledger_maintenance_assessment::{
    assess_ledger_maintenance, LedgerMaintenanceAssessmentError, LedgerMaintenanceAssessmentPolicy,
};
use layer2_core::sandbox::store::ledger_maintenance::LedgerMaintenanceReader;
use layer2_core::SandboxId;

use super::{problem_document, problem_response};
use crate::AuthenticatedCaller;

#[path = "ledger_maintenance_shapes.rs"]
mod shapes;
use shapes::LedgerMaintenanceAssessmentView;

#[derive(Clone)]
struct AssessmentState {
    reader: Arc<dyn LedgerMaintenanceReader>,
    policy: LedgerMaintenanceAssessmentPolicy,
}

/// Composition must explicitly enable the policy. This router provides no
/// maintenance admission, recovery, lease, deletion or native-effect operation.
pub fn ledger_maintenance_router(
    reader: Arc<dyn LedgerMaintenanceReader>,
    policy: LedgerMaintenanceAssessmentPolicy,
) -> Router {
    Router::new()
        .route("/api/v1/sandboxes/{id}/ledger-maintenance", get(assess))
        .layer(axum::middleware::from_fn(private_headers))
        .with_state(AssessmentState { reader, policy })
}

/// Inspect retained maintenance and recovery metadata without admitting work.
/// The response does not prove native-effect absence, backup validity or exclusion.
#[utoipa::path(get, path = "/api/v1/sandboxes/{id}/ledger-maintenance", tag = "sandboxes",
    params(("id" = String, Path)),
    responses((status = 200, body = LedgerMaintenanceAssessmentView),
        (status = 401, body = crate::rest::ProblemDocument),
        (status = 404, body = crate::rest::ProblemDocument),
        (status = 503, body = crate::rest::ProblemDocument)))]
async fn assess(
    State(state): State<AssessmentState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
) -> Response {
    if id.is_empty() || id.len() > 256 || id.trim() != id || id.chars().any(char::is_control) {
        return refusal(LedgerMaintenanceAssessmentError::NotFound);
    }
    match assess_ledger_maintenance(state.reader.as_ref(), state.policy, &caller, &SandboxId(id))
        .await
    {
        Ok(value) => Json(LedgerMaintenanceAssessmentView::from(value)).into_response(),
        Err(error) => refusal(error),
    }
}

fn refusal(error: LedgerMaintenanceAssessmentError) -> Response {
    use LedgerMaintenanceAssessmentError::*;
    let (status, slug, code) = match error {
        Disabled => (
            StatusCode::NOT_FOUND,
            "ledger-maintenance-disabled",
            "ledger_maintenance_disabled",
        ),
        NotFound => (StatusCode::NOT_FOUND, "not-found", "not_found"),
        InvalidSource => (
            StatusCode::SERVICE_UNAVAILABLE,
            "ledger-maintenance-inconsistent",
            "ledger_maintenance_inconsistent",
        ),
        Unavailable => (
            StatusCode::SERVICE_UNAVAILABLE,
            "ledger-maintenance-unavailable",
            "ledger_maintenance_unavailable",
        ),
    };
    problem_response(
        status,
        problem_document(status, slug, &error.to_string(), code),
    )
}

async fn private_headers(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "private, no-store".parse().unwrap());
    response
}

pub fn ledger_maintenance_document() -> utoipa::openapi::OpenApi {
    use utoipa::OpenApi;
    #[derive(OpenApi)]
    #[openapi(
        paths(assess),
        components(schemas(
            LedgerMaintenanceAssessmentView,
            shapes::LedgerRecoveryCountsView,
            shapes::LedgerMaintenancePhaseView
        ))
    )]
    struct Document;
    Document::openapi()
}

#[cfg(test)]
#[path = "ledger_maintenance_tests.rs"]
mod tests;
