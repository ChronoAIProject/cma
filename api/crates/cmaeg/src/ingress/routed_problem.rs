//! Stable refusal mapping for durable admission storage errors.
use super::{AdmissionError, Problem, ProblemCode};

pub(super) fn admission_problem(error: AdmissionError) -> Problem {
    let (code, status, detail) = match error {
        AdmissionError::Conflict => (ProblemCode::Conflict, 409, "routed_event_conflict"),
        AdmissionError::GuardConflict => (
            ProblemCode::ProviderUnavailable,
            503,
            "routed_authority_changed_retry",
        ),
        AdmissionError::TooLarge => (ProblemCode::PayloadTooLarge, 413, "routed_event_too_large"),
        AdmissionError::Invalid => (ProblemCode::InvalidRequest, 400, "routed_event_invalid"),
        AdmissionError::Expired => (ProblemCode::EventRefExpired, 410, "routed_event_expired"),
        _ => (
            ProblemCode::ProviderUnavailable,
            503,
            "routed_admission_unavailable",
        ),
    };
    Problem::new(code, status, detail)
}
