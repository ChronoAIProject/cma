use axum::{
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use layer2_core::bridge::invoke::BridgeInvokeError;
pub(super) struct InvokeProblem(pub BridgeInvokeError);
impl IntoResponse for InvokeProblem {
    fn into_response(self) -> Response {
        use BridgeInvokeError::*;
        let (status,code,detail)=match self.0 {
            Bridge(error)=>return super::super::bridges::problem::BridgeProblem(error).into_response(),
            NotFound=>(StatusCode::NOT_FOUND,"bridge_invoke_not_found","Invoke grant not found."),
            Forbidden=>(StatusCode::FORBIDDEN,"bridge_invoke_management_required","Current owner or organization administrator and retained Agent management authority are required."),
            Conflict=>(StatusCode::CONFLICT,"bridge_invoke_revision_conflict","Grant or Bridge state changed. Read the current revisions before a new command."),
            IdempotencyConflict=>(StatusCode::CONFLICT,"idempotency_key_reused","This key already names a different explicit grant command."),
            Revoked=>(StatusCode::CONFLICT,"bridge_invoke_revoked","This grant was revoked. New explicit consent requires a new grant."),
            TargetMismatch=>(StatusCode::CONFLICT,"bridge_invoke_target_mismatch","The exact retained Agent creation identity and Bridge owner must match."),
            Suspended=>(StatusCode::CONFLICT,"bridge_invoke_suspended","The Bridge is suspended for this operation."),
            Invalid(_)=>(StatusCode::BAD_REQUEST,"invalid_bridge_invoke_request","Provide valid grant identifiers, exact revisions, explicit shared-context consent and a limit from 1 through 100."),
            Unavailable|Corrupt=>(StatusCode::SERVICE_UNAVAILABLE,"bridge_invoke_unavailable","Current invoke grant authority is unavailable. Retry the same command after refreshing current state."),
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
