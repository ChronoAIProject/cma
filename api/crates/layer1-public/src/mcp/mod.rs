//! Documented MCP endpoint, explicitly unavailable until its protocol is implemented.

mod agent_access;
pub use agent_access::agent_access_router;
pub use agent_access::agent_delivery_tools_router;
pub use agent_access::agent_tools_router;

use axum::{
    http::{header, HeaderValue, StatusCode},
    response::Response,
    routing::post,
    Router,
};

use crate::rest::{problem_document, problem_response};
use crate::{InterfaceProfile, Operation};

#[derive(Debug, Default)]
pub struct McpInterface;

impl InterfaceProfile for McpInterface {
    fn interface_id(&self) -> &'static str {
        "mcp"
    }

    fn operations(&self) -> &'static [Operation] {
        &[]
    }
}

pub fn router() -> Router {
    Router::new().route("/api/v1/mcp", post(not_available))
}

async fn not_available() -> Response {
    let status = StatusCode::NOT_IMPLEMENTED;
    let mut body = problem_document(
        status,
        "interface-not-available",
        "The mcp interface is not yet available.",
        "interface_not_available",
    );
    body["interface"] = "mcp".into();
    let mut response = problem_response(status, body);
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

#[cfg(test)]
mod tests;
