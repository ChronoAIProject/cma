use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};
use layer2_core::resource_access::{
    AccessError, MembershipError, ResourceAccessError, ResourceAccessStoreError,
};

pub(crate) struct ResourceProblem(pub ResourceAccessError);
impl From<ResourceAccessError> for ResourceProblem {
    fn from(value: ResourceAccessError) -> Self {
        Self(value)
    }
}
impl IntoResponse for ResourceProblem {
    fn into_response(self) -> Response {
        super::dependency_diagnostics::resource_failure(&self.0);
        use ResourceAccessError::*;
        let (status, code, detail) = match self.0 {
            AccountDeletionInProgress => (StatusCode::FORBIDDEN, "account_deletion_in_progress", "Account deletion is in progress."),
            Store(ResourceAccessStoreError::ShareRecipientUnavailable) => (StatusCode::CONFLICT, "share_recipient_unavailable", "The share recipient is unavailable."),
            Access(AccessError::NotFound) | Store(ResourceAccessStoreError::NotFound) => (StatusCode::NOT_FOUND, "resource_not_found", "Resource not found."),
            Access(AccessError::Forbidden) | Membership(MembershipError::NotMember) => (StatusCode::FORBIDDEN, "resource_forbidden", "Current authority does not permit this operation."),
            Access(AccessError::UnsafeInteractiveShare) | Store(ResourceAccessStoreError::UnsafeInteractiveShare) => (StatusCode::CONFLICT, "unsafe_interactive_share", "Interactive Agent sharing requires independent isolation or an explicit compatible whole-Sandbox grant. Use view sharing for this Agent."),
            Store(ResourceAccessStoreError::Conflict) => (StatusCode::CONFLICT, "resource_access_changed", "Resource access changed. Read current access and retry with the current revision."),
            Store(ResourceAccessStoreError::IdempotencyConflict) => (StatusCode::CONFLICT, "idempotency_key_reused", "The idempotency key was used for another share command."),
            Store(ResourceAccessStoreError::Invalid) => (StatusCode::BAD_REQUEST, "invalid_share_command", "Share command or resource selector is invalid."),
            Store(ResourceAccessStoreError::Capacity) => (StatusCode::TOO_MANY_REQUESTS, "share_capacity", "This Sandbox has reached its active-share limit."),
            Access(AccessError::RevalidationRequired) | Membership(_) => (StatusCode::SERVICE_UNAVAILABLE, "organization_authority_unavailable", "Current organization authorization is unavailable. Retry after the identity provider recovers."),
            Store(ResourceAccessStoreError::Unavailable | ResourceAccessStoreError::Corrupt) => (StatusCode::SERVICE_UNAVAILABLE, "resource_access_unavailable", "Resource access is temporarily unavailable. Retry the same command key to recover an unacknowledged result."),
        };
        super::problem_response(status, super::problem_document(status, code, detail, code))
    }
}
