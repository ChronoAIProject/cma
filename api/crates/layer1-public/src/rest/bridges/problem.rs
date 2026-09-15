use axum::{
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use layer2_core::bridge::{BridgeError, BridgeSetupBlock};

pub(crate) struct BridgeProblem(pub BridgeError);
impl From<BridgeError> for BridgeProblem {
    fn from(value: BridgeError) -> Self {
        Self(value)
    }
}
impl IntoResponse for BridgeProblem {
    fn into_response(self) -> Response {
        use BridgeError::*;
        let (status,code,detail)=match self.0 {
            NotFound => (StatusCode::NOT_FOUND,"bridge_not_found","Bridge not found."),
            Forbidden => (StatusCode::FORBIDDEN,"bridge_management_required","Current owner or organization administrator authority is required."),
            InvalidConfiguration(_) => (StatusCode::BAD_REQUEST,"invalid_bridge_configuration","The bridge configuration or command is invalid. Select explicit compatible partition, placement, admission and bounded limits."),
            Conflict => (StatusCode::CONFLICT,"bridge_revision_conflict","Bridge state changed. Read the current revision before a new command."),
            IdempotencyConflict => (StatusCode::CONFLICT,"idempotency_key_reused","This key or selected custody identity already names different bridge input."),
            ImmutableIdentity => (StatusCode::CONFLICT,"bridge_identity_immutable","Owner, bot and platform identity require a new bridge."),
            RebindingPolicyRequired => (StatusCode::BAD_REQUEST,"bridge_rebinding_policy_required","Select a policy for accepted work before changing the fixed Agent target."),
            Retired => (StatusCode::CONFLICT,"bridge_retired","This bridge is retired; retained Agents and history are preserved."),
            AdmissionStopped => (StatusCode::CONFLICT,"bridge_admission_stopped","This bridge is not accepting new work."),
            HistoryLimit => (StatusCode::CONFLICT,"bridge_history_limit","This bridge has reached its supported configuration history limit."),
            ConnectionRequired(reason) => match reason {
                BridgeSetupBlock::BindingMismatch => (StatusCode::CONFLICT,"bridge_binding_mismatch","The selected owner, bot, key and routes do not form a currently verified binding."),
                BridgeSetupBlock::CallbackConflict => (StatusCode::CONFLICT,"bridge_callback_conflict","The selected key callback or other active routes conflict with this bridge. Configure a dedicated matching key without changing unrelated routes."),
                BridgeSetupBlock::CustodyUnavailable => (StatusCode::CONFLICT,"bridge_custody_required","Import the exact active selected Agent key into CMA bridge custody."),
                BridgeSetupBlock::RenewableAuthorityUnavailable => (StatusCode::CONFLICT,"bridge_renewable_authority_required","New Sandbox placement requires verified distinct scoped credential issuance and renewal."),
                BridgeSetupBlock::TargetMismatch => (StatusCode::CONFLICT,"bridge_target_mismatch","The retained Agent target must have the exact selected lineage and compatible owner."),
                BridgeSetupBlock::InvokeConsentRequired => (StatusCode::CONFLICT,"bridge_invoke_consent_required","Issue an explicit current-generation grant for the exact retained Agent before enabling this Bridge."),
                BridgeSetupBlock::ProfileUnavailable => (StatusCode::CONFLICT,"bridge_profile_unavailable","The exact selected profile revisions are unavailable or incompatible."),
                BridgeSetupBlock::AuthorityExpired => (StatusCode::SERVICE_UNAVAILABLE,"bridge_authority_unavailable","Current bridge authority expired during verification. Retry after refreshing current state."),
            },
            AuthorityUnavailable => (StatusCode::SERVICE_UNAVAILABLE,"bridge_authority_unavailable","Current owner or provider authority is unavailable."),
            Unavailable|Corrupt => (StatusCode::SERVICE_UNAVAILABLE,"bridge_unavailable","Bridge state is temporarily unavailable. Retry the same request to recover an unacknowledged command."),
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
