use axum::http::StatusCode;
use layer2_core::sandbox::model_credit::FundingError;

pub(crate) fn projection(
    reason: FundingError,
) -> (StatusCode, &'static str, &'static str, &'static str) {
    match reason {
        FundingError::Unsupported => (
            StatusCode::SERVICE_UNAVAILABLE,
            "model-funding-unsupported",
            "Guaranteed model funding is not supported for this route. Keep the original request key and check its status before submitting again.",
            "model_funding_unsupported",
        ),
        FundingError::Insufficient => (
            StatusCode::PAYMENT_REQUIRED,
            "model-funding-insufficient",
            "Model funding is insufficient for this turn. Contact the person who manages model funding. Keep the original request key and check its status before submitting again.",
            "model_funding_insufficient",
        ),
        FundingError::Unavailable => (
            StatusCode::SERVICE_UNAVAILABLE,
            "model-funding-unavailable",
            "Model funding could not be verified. Check the original request status with the same key; do not submit a replacement request.",
            "model_funding_unavailable",
        ),
        FundingError::Expired => (
            StatusCode::CONFLICT,
            "model-funding-expired",
            "The model funding reservation has expired. Check the original request status before requesting funding again.",
            "model_funding_expired",
        ),
        FundingError::BindingChanged => (
            StatusCode::CONFLICT,
            "model-funding-binding-changed",
            "Model funding no longer matches the current selection. Check the original request status and review the selection before requesting funding again.",
            "model_funding_binding_changed",
        ),
        FundingError::CheckpointChanged => (
            StatusCode::CONFLICT,
            "model-funding-checkpoint-conflict",
            "The model funding checkpoint changed. Read the original request status before taking further action.",
            "model_funding_checkpoint_conflict",
        ),
        // Absence alone proves neither admission refusal nor settlement finality.
        FundingError::NoReservation | FundingError::ReconciliationRequired => (
            StatusCode::SERVICE_UNAVAILABLE,
            "model-funding-reconciliation-required",
            "The original model request requires reconciliation. Check its status with the same key; do not submit a replacement request.",
            "model_funding_reconciliation_required",
        ),
        FundingError::Forbidden => (
            StatusCode::FORBIDDEN,
            "sandbox-use-forbidden",
            "This operation requires authority to use the caller's sandboxes.",
            "forbidden",
        ),
    }
}
