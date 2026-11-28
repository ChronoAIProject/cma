use axum::{http::StatusCode, response::Response};
use layer2_core::budget::BudgetError;
use std::time::{Duration, UNIX_EPOCH};

pub(crate) fn owner_reset_guidance(reset_at: u64) -> String {
    let reset = layer2_core::rfc3339_at(UNIX_EPOCH + Duration::from_secs(reset_at));
    format!(
        "HTTP 402: The owner's model budget is exhausted. The daily window resets at {reset}. Outstanding reservations still count. After budget is available, send a new message to continue. Completed work is preserved and this turn will not replay automatically."
    )
}

pub(crate) fn owner_exhausted(reset_at: u64) -> Response {
    let status = StatusCode::PAYMENT_REQUIRED;
    let body = crate::rest::problem_document(
        status,
        "model-budget-refused",
        &owner_reset_guidance(reset_at),
        BudgetError::OwnerExhausted.code(),
    );
    crate::rest::problem_response(status, body)
}
