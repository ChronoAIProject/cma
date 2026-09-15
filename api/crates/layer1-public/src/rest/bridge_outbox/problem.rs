use axum::{
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use layer2_core::bridge::outbox::{management::OutboxManagementError, OutboxError};
pub(super) struct OutboxProblem(pub OutboxManagementError);
impl IntoResponse for OutboxProblem {
    fn into_response(self) -> Response {
        use OutboxManagementError::*;
        let (status, code, detail) = match self.0 {
            Bridge(error) => return super::super::bridges::problem::BridgeProblem(error).into_response(),
            NotFound => (StatusCode::NOT_FOUND,"bridge_delivery_not_found","Delivery not found in this Bridge."),
            Invalid => (StatusCode::BAD_REQUEST,"invalid_bridge_delivery_request","Provide an original delivery identity, current item and Bridge revisions, and an explicit recovery action."),
            DuplicateRiskRequired => (StatusCode::BAD_REQUEST,"bridge_delivery_duplicate_risk_required","The earlier send may have reached the recipient. Explicitly accept possible duplicate delivery before retrying."),
            Delivery(error) => match error {
                OutboxError::Conflict => (StatusCode::CONFLICT,"bridge_delivery_revision_conflict","Delivery or Bridge state changed. Read current revisions before a new command."),
                OutboxError::Suspended => (StatusCode::CONFLICT,"bridge_delivery_suspended","Outbound delivery is suspended for the original Bridge configuration."),
                OutboxError::Expired => (StatusCode::CONFLICT,"bridge_delivery_expired","The original reply audience has expired and cannot be retargeted."),
                OutboxError::Bounds => (StatusCode::CONFLICT,"bridge_delivery_attempt_limit","This delivery reached its supported attempt limit."),
                OutboxError::SourceMismatch | OutboxError::RetainedKeyRequired | OutboxError::CapabilityUnavailable => (StatusCode::CONFLICT,"bridge_delivery_authority_required","A currently verified retained key for the original binding and audience is required."),
                OutboxError::RequestConflict => (StatusCode::CONFLICT,"idempotency_key_reused","This request key already identifies different recovery input."),
                _ => (StatusCode::SERVICE_UNAVAILABLE,"bridge_delivery_unavailable","Current delivery state or authority is unavailable. Retry the identical command key to recover an uncertain commit."),
            },
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
