//! MCP interface (priority 4) — for agent clients.
//!
//! A projection of the Layer 2 core session model into MCP idiom.

use axum::{response::Response, routing::post, Router};

use crate::unavailable::interface_unavailable;
use crate::{InterfaceProfile, Operation};

/// The MCP projection of the core session model.
#[derive(Debug, Default)]
pub struct McpInterface;

impl InterfaceProfile for McpInterface {
    fn interface_id(&self) -> &'static str {
        "mcp"
    }

    fn operations(&self) -> &'static [Operation] {
        &[
            Operation::CreateSession,
            Operation::SendInput,
            Operation::ResolveApproval,
        ]
    }
}

/// The documented MCP endpoint, refusing by name until it is implemented.
pub fn router() -> Router {
    Router::new().route("/api/v1/mcp", post(not_available))
}

async fn not_available() -> Response {
    interface_unavailable("mcp")
}

#[cfg(test)]
mod tests;
