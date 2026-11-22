//! RFC 9457 projection of generic core and HTTP failures.

use axum::{
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use layer2_core::CoreError;
use serde::Serialize;
use utoipa::ToSchema;

use super::PROBLEM_BASE;

pub(super) const SANDBOX_CREDENTIAL_SETUP_DETAIL: &str = "Sign in to CMA through /auth/login with this same account to authorize renewable Sandbox credentials. For keyed operations, retry this refused command with a new Idempotency-Key after sign-in succeeds.";

/// RFC 9457 problem details returned by REST refusals.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct ProblemDocument {
    #[serde(rename = "type")]
    pub problem_type: String,
    pub title: String,
    pub status: u16,
    pub detail: String,
    pub code: String,
}

pub(crate) fn problem_document(
    status: StatusCode,
    slug: &str,
    detail: &str,
    code: &str,
) -> serde_json::Value {
    serde_json::to_value(ProblemDocument {
        problem_type: format!("{PROBLEM_BASE}/{slug}"),
        title: slug.replace('-', " "),
        status: status.as_u16(),
        detail: detail.to_owned(),
        code: code.to_owned(),
    })
    .expect("a problem document contains only JSON values")
}

pub(crate) fn problem_response(status: StatusCode, body: serde_json::Value) -> Response {
    let mut response = (status, Json(body)).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/problem+json"),
    );
    response
}

/// HTTP-level failures raised before a core operation begins.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpProblem {
    RequestBudgetUnavailable,
    RequestTooLarge,
    RateLimited { retry_after_seconds: u64 },
    PublicStreamCapacityExhausted,
    RuntimeRequestCapacityExhausted,
    RuntimeStreamCapacityExhausted,
    SandboxBackendUnavailable,
    RelayObserverCapacityExhausted,
    RelayCapacityExhausted,
    BufferedReadCapacityExhausted,
    NotFound,
    MethodNotAllowed,
}

impl IntoResponse for HttpProblem {
    fn into_response(self) -> Response {
        let limited = match self {
            Self::RateLimited { retry_after_seconds } => Some((
                retry_after_seconds,
                "rate-limited",
                "This caller has exhausted the request budget for this route.",
                "rate_limited",
            )),
            Self::BufferedReadCapacityExhausted => Some((
                1,
                "buffered-read-capacity-exhausted",
                "Shared read capacity is busy. Retry this read after the indicated delay.",
                "buffered_read_capacity_exhausted",
            )),
            Self::RuntimeRequestCapacityExhausted => Some((1, "runtime-request-capacity-exhausted", "Runtime request capacity is busy. Retry after the indicated delay.", "runtime_request_capacity_exhausted")),
            Self::RuntimeStreamCapacityExhausted => Some((1, "runtime-stream-capacity-exhausted", "Runtime stream capacity is busy. Existing work continues.", "runtime_stream_capacity_exhausted")),
            Self::RelayObserverCapacityExhausted => Some((1, "relay-observer-capacity-exhausted", "Observation relay capacity is busy. Reconnect with the durable cursor.", "relay_observer_capacity_exhausted")),
            Self::RelayCapacityExhausted => Some((1, "relay-capacity-exhausted", "Agent relay capacity is busy. Retry the original operation after the indicated delay.", "relay_capacity_exhausted")),
            Self::PublicStreamCapacityExhausted => Some((
                1,
                "public-stream-capacity-exhausted",
                "Public observation capacity is busy. Retry with the original request key or cursor.",
                "public_stream_capacity_exhausted",
            )),
            _ => None,
        };
        if let Some((retry_after_seconds, slug, detail, code)) = limited {
            let mut response = problem_response(
                StatusCode::TOO_MANY_REQUESTS,
                problem_document(StatusCode::TOO_MANY_REQUESTS, slug, detail, code),
            );
            response.headers_mut().insert(
                header::RETRY_AFTER,
                HeaderValue::from_str(&retry_after_seconds.max(1).to_string())
                    .expect("a positive integer is a valid Retry-After value"),
            );
            return response;
        }
        let (status, slug, detail, code) = match self {
            Self::RequestBudgetUnavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                "request-budget-unavailable",
                "Request capacity is temporarily unavailable.",
                "request_budget_unavailable",
            ),
            Self::SandboxBackendUnavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                "sandbox-backend-unavailable",
                "Sandbox execution is unavailable in this deployment.",
                "sandbox_backend_unavailable",
            ),
            Self::RequestTooLarge => (
                StatusCode::PAYLOAD_TOO_LARGE,
                "request-too-large",
                "The request body exceeds what this service accepts.",
                "request_too_large",
            ),
            Self::NotFound => (
                StatusCode::NOT_FOUND,
                "not-found",
                "No REST route matches this request path.",
                "not_found",
            ),
            Self::MethodNotAllowed => (
                StatusCode::METHOD_NOT_ALLOWED,
                "method-not-allowed",
                "This REST route does not accept the request method.",
                "method_not_allowed",
            ),
            Self::RateLimited { .. }
            | Self::PublicStreamCapacityExhausted
            | Self::RuntimeRequestCapacityExhausted
            | Self::RuntimeStreamCapacityExhausted
            | Self::RelayObserverCapacityExhausted
            | Self::RelayCapacityExhausted
            | Self::BufferedReadCapacityExhausted => {
                unreachable!("handled above")
            }
        };
        problem_response(status, problem_document(status, slug, detail, code))
    }
}

/// A generic core failure rendered as a problem document.
pub struct Problem(pub CoreError);

impl IntoResponse for Problem {
    fn into_response(self) -> Response {
        let (status, slug) = match &self.0 {
            CoreError::AdmissionPaused => return super::admission_problem::paused(),
            CoreError::AdmissionCapacity(_) => {
                return problem_response(
                    StatusCode::SERVICE_UNAVAILABLE,
                    problem_document(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "runtime-unavailable",
                        "A required service is unavailable.",
                        "runtime_unavailable",
                    ),
                );
            }
            CoreError::AdmissionControlWritesUnavailable => {
                return super::admission_problem::writes_unavailable()
            }
            CoreError::AdmissionCeilingsWritesUnavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                "admission-ceilings-writes-unavailable",
            ),
            CoreError::Forbidden => (StatusCode::FORBIDDEN, "forbidden"),
            CoreError::InvalidRequest(_) => (StatusCode::UNPROCESSABLE_ENTITY, "invalid-request"),
            CoreError::InvalidState(_) => (StatusCode::CONFLICT, "invalid-state"),
            CoreError::RuntimeUnavailable(_) => {
                (StatusCode::SERVICE_UNAVAILABLE, "runtime-unavailable")
            }
            CoreError::RuntimeRefused(_) => (StatusCode::SERVICE_UNAVAILABLE, "runtime-refused"),
            CoreError::ServiceNotGranted(_) => {
                (StatusCode::UNPROCESSABLE_ENTITY, "service-not-granted")
            }
            CoreError::ServiceCatalogUnavailable(_) => (
                StatusCode::SERVICE_UNAVAILABLE,
                "service-catalogue-unavailable",
            ),
            CoreError::SkillCatalogUnavailable(_) => (
                StatusCode::SERVICE_UNAVAILABLE,
                "skill-catalogue-unavailable",
            ),
            CoreError::ModelBudgetWritesUnavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                "model-budget-writes-unavailable",
            ),
            CoreError::InvalidPressurePolicy(_) => {
                (StatusCode::UNPROCESSABLE_ENTITY, "invalid-pressure-policy")
            }
        };
        problem_response(
            status,
            problem_document(status, slug, &self.0.to_string(), self.0.code()),
        )
    }
}

/// Project transient capacity without exposing occupancy or deployment shape.
pub(crate) fn runtime_capacity(capacity: layer2_core::sandbox::RuntimeCapacityError) -> Response {
    match capacity {
        layer2_core::sandbox::RuntimeCapacityError::Stream => {
            HttpProblem::RuntimeStreamCapacityExhausted
        }
        layer2_core::sandbox::RuntimeCapacityError::Request => {
            HttpProblem::RuntimeRequestCapacityExhausted
        }
    }
    .into_response()
}

#[cfg(test)]
#[path = "credential_prerequisite_problem_tests.rs"]
mod credential_prerequisite_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_http_problem_is_machine_readable() {
        for problem in [
            HttpProblem::RequestTooLarge,
            HttpProblem::RateLimited {
                retry_after_seconds: 3,
            },
            HttpProblem::BufferedReadCapacityExhausted,
            HttpProblem::NotFound,
            HttpProblem::MethodNotAllowed,
        ] {
            let response = problem.into_response();
            assert_eq!(
                response.headers().get(header::CONTENT_TYPE).unwrap(),
                "application/problem+json"
            );
        }
    }
}
