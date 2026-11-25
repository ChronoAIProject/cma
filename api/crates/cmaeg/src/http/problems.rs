//! Gateway problem responses and readiness diagnostics.
use axum::{http::StatusCode, response::Response};
use cmaeg_core::problem::ProblemCode;
pub(crate) use cmaeg_http::http::problems::problem_document;
pub use cmaeg_http::http::problems::{no_store, problem, PROTOCOL_HEADER, PROTOCOL_VERSION};
use cmaeg_http::http::problems::{protocol_response, with_protocol_headers};

/// Answers a request whose handler or middleware panicked. The payload is
/// dropped unread: a panic message can carry request data, which must reach
/// neither a log nor the caller.
pub(crate) fn panic_response(_: Box<dyn std::any::Any + Send + 'static>) -> Response {
    tracing::error!("request handler panicked");
    // The panic unwound past `protocol_headers`, so its headers are added here.
    crate::telemetry::http_failure::FailureStage::Panic.response(with_protocol_headers(problem(
        StatusCode::INTERNAL_SERVER_ERROR,
        ProblemCode::Internal,
        "internal_error",
    )))
}

pub async fn protocol_headers(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let response = next.run(request).await;
    let is_readiness_report = response
        .extensions()
        .get::<super::readiness::ReadinessReport>()
        .is_some();
    protocol_response(response, is_readiness_report)
}
