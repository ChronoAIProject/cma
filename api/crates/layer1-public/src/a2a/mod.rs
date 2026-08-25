//! A2A interface (priority 5) — for agent clients.
//!
//! A projection of the Layer 2 core session model into A2A idiom.

use axum::{response::Response, routing::any, routing::get, Router};

use crate::unavailable::interface_unavailable;
use crate::{InterfaceProfile, Operation};

/// The A2A projection of the core session model.
#[derive(Debug, Default)]
pub struct A2aInterface;

impl InterfaceProfile for A2aInterface {
    fn interface_id(&self) -> &'static str {
        "a2a"
    }

    fn operations(&self) -> &'static [Operation] {
        &[
            Operation::CreateSession,
            Operation::SendInput,
            Operation::StreamEvents,
            Operation::ResolveApproval,
            Operation::Interrupt,
            Operation::GetUsage,
        ]
    }
}

/// Routes that name the documented A2A surface without claiming to serve it.
pub fn router() -> Router {
    Router::new()
        .route("/.well-known/agent-card.json", get(not_available))
        .route("/api/v1/a2a", any(not_available))
}

async fn not_available() -> Response {
    interface_unavailable("a2a")
}

#[cfg(test)]
mod tests;
