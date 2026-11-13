//! Stable refusal mapping for durable admission storage errors.
use super::{AdmissionError, Problem, ProblemCode};

pub(super) fn admission_problem(error: AdmissionError) -> Problem {
    let class = crate::ingress::admission_trace::admission_class(&error);
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
    tracing::info!(
        event = "eg_admission_public_mapping",
        class,
        public_status = status,
        public_code = %code,
        public_detail = detail,
        "Admission error mapped to public refusal"
    );
    Problem::new(code, status, detail)
}
