//! Stable RFC 9457 responses and protocol headers.

use axum::{
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use cmaeg_core::problem::{Problem, ProblemCode};
use cmaeg_engine::RefusalReason;

pub const PROTOCOL_HEADER: &str = "CMAEG-Protocol-Version";
pub const PROTOCOL_VERSION: &str = "1";

pub fn problem(status: StatusCode, code: ProblemCode, detail: impl Into<String>) -> Response {
    problem_document(Problem::new(code, status.as_u16(), detail))
}

pub fn problem_document(body: Problem) -> Response {
    let label = serde_json::to_value(body.code)
        .unwrap()
        .as_str()
        .unwrap()
        .to_owned();
    let status = StatusCode::from_u16(body.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let mut response = (status, Json(body)).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/problem+json"),
    );
    response.extensions_mut().insert(RefusalReason(label));
    response
}

pub fn no_store(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

pub fn protocol_response(mut response: Response, preserve_response_body: bool) -> Response {
    if !preserve_response_body
        && (response.status().is_client_error() || response.status().is_server_error())
        && response
            .headers()
            .get(header::CONTENT_TYPE)
            .is_none_or(|value| value != "application/problem+json")
    {
        let status = response.status();
        let allow = response.headers().get(header::ALLOW).cloned();
        response = problem(
            status,
            match status {
                StatusCode::PAYLOAD_TOO_LARGE => ProblemCode::PayloadTooLarge,
                StatusCode::UNSUPPORTED_MEDIA_TYPE => ProblemCode::UnsupportedMediaType,
                _ if status.is_server_error() => ProblemCode::Internal,
                _ => ProblemCode::InvalidRequest,
            },
            status.canonical_reason().unwrap_or("request failed"),
        );
        if let Some(allow) = allow {
            response.headers_mut().insert(header::ALLOW, allow);
        }
    }
    with_protocol_headers(response)
}

pub fn with_protocol_headers(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(PROTOCOL_HEADER, HeaderValue::from_static(PROTOCOL_VERSION));
    no_store(response)
}
