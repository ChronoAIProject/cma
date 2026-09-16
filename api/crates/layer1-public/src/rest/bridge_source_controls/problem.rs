use axum::{
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use layer2_core::bridge::invoke::BridgeInvokeError;
pub(super) struct SourceControlProblem(pub BridgeInvokeError);
impl IntoResponse for SourceControlProblem {
    fn into_response(self) -> Response {
        use BridgeInvokeError::*;
        let (status, code, detail) = match self.0 {
            Bridge(error) => return super::super::bridges::problem::BridgeProblem(error).into_response(),
            NotFound => (StatusCode::NOT_FOUND, "bridge_invoke_not_found", "Invoke grant not found."),
            Forbidden => (StatusCode::FORBIDDEN, "bridge_source_control_management_required", "Current Bridge management and exact Agent management and interaction authority are required."),
            Conflict => (StatusCode::CONFLICT, "bridge_source_control_revision_conflict", "Consent or current resource revisions changed. Retain the original key and body to recover uncertain acceptance."),
            IdempotencyConflict => (StatusCode::CONFLICT, "idempotency_key_reused", "This key already names a different explicit source-control consent request."),
            Revoked => (StatusCode::CONFLICT, "bridge_invoke_revoked", "This invoke grant was revoked and cannot receive new sender-control consent."),
            TargetMismatch => (StatusCode::CONFLICT, "bridge_invoke_target_mismatch", "The exact retained Agent creation identity and Bridge owner must match."),
            Suspended => (StatusCode::CONFLICT, "bridge_invoke_suspended", "The Bridge is suspended for new sender-control consent."),
            Invalid(_) => (StatusCode::BAD_REQUEST, "invalid_bridge_source_control_request", "Provide valid grant identifiers, exact Bridge, grant and policy revisions, and an explicit sender-control consent action."),
            Unavailable | Corrupt => (StatusCode::SERVICE_UNAVAILABLE, "bridge_source_control_unavailable", "Current consent state or authority is unavailable. Recover uncertain acceptance with the identical key and body."),
        };
        let mut response = super::super::problem_response(
            status,
            super::super::problem_document(status, code, detail, code),
        );
        if status == StatusCode::SERVICE_UNAVAILABLE {
            response
                .headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
        }
        response
    }
}
