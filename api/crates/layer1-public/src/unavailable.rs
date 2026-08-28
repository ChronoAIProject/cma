//! HTTP refusal shared by public interfaces that are declared but not served.

use axum::{
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};

/// Name an unavailable interface in the common problem-document shape.
pub(crate) fn interface_unavailable(interface: &'static str) -> Response {
    let status = StatusCode::NOT_IMPLEMENTED;
    let body = serde_json::json!({
        "type": "https://spec.chrono-ai.fun/cma/1.0/problems/interface-not-available",
        "title": "interface not available",
        "status": status.as_u16(),
        "detail": format!("The {interface} interface is not yet available."),
        "code": "interface_not_available",
        "interface": interface,
    });
    let mut response = (status, Json(body)).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/problem+json"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}
