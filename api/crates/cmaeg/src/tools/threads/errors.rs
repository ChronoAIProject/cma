//! Shared lifecycle refusal and platform outcome mapping.
use super::*;

pub(super) fn invalid_reference() -> Problem {
    Problem::new(
        ProblemCode::EventRefInvalid,
        400,
        "thread_reference_invalid_or_expired",
    )
}
pub(super) fn busy() -> Problem {
    Problem::new(ProblemCode::RateLimited, 429, "thread_lifecycle_busy")
}
pub(super) fn revoked() -> ThreadOutcome {
    ThreadOutcome::Rejected {
        reason: "thread_authority_revoked".into(),
    }
}
pub(super) fn outcome(error: RestError) -> ThreadOutcome {
    match error {
        RestError::RateLimited(retry_after) => ThreadOutcome::RateLimited { retry_after },
        RestError::Unknown => ThreadOutcome::Unknown {
            reason: "thread_write_unconfirmed".into(),
        },
        RestError::Unavailable => ThreadOutcome::Rejected {
            reason: "thread_preflight_unavailable".into(),
        },
        RestError::Rejected | RestError::NotFound => ThreadOutcome::Rejected {
            reason: "thread_permission_or_target_unavailable".into(),
        },
    }
}
pub(super) fn read_problem(error: RestError) -> Problem {
    match error {
        RestError::RateLimited(_) => {
            Problem::new(ProblemCode::RateLimited, 429, "thread_read_rate_limited")
        }
        RestError::Rejected | RestError::NotFound => Problem::new(
            ProblemCode::Forbidden,
            403,
            "thread_permission_or_target_unavailable",
        ),
        _ => unavailable(),
    }
}
