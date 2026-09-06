//! Documented A2A routes, explicitly unavailable until its protocol is implemented.

use axum::{
    http::{header, HeaderValue, StatusCode},
    response::Response,
    routing::{any, get},
    Router,
};

use crate::rest::{problem_document, problem_response};
use crate::{InterfaceProfile, Operation};

#[derive(Debug, Default)]
pub struct A2aInterface;

impl InterfaceProfile for A2aInterface {
    fn interface_id(&self) -> &'static str {
        "a2a"
    }

    fn operations(&self) -> &'static [Operation] {
        &[]
    }
}

pub fn router() -> Router {
    Router::new()
        .route("/.well-known/agent-card.json", get(not_available))
        .route("/api/v1/a2a", any(not_available))
}

async fn not_available() -> Response {
    let status = StatusCode::NOT_IMPLEMENTED;
    let mut body = problem_document(
        status,
        "interface-not-available",
        "The a2a interface is not yet available.",
        "interface_not_available",
    );
    body["interface"] = "a2a".into();
    let mut response = problem_response(status, body);
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

#[cfg(test)]
mod tests;
