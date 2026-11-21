//! Public provider discovery; the A2A message protocol remains unavailable.

use axum::{
    extract::State,
    http::{header, HeaderValue, StatusCode},
    response::Response,
    routing::{any, get},
    Json, Router,
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

pub fn router(public_base: &str) -> Router {
    Router::new()
        .route("/.well-known/agent-card.json", get(agent_card))
        .route("/api/v1/a2a", any(not_available))
        .with_state(public_base.trim_end_matches('/').to_owned())
}
async fn agent_card(State(public_base): State<String>) -> impl axum::response::IntoResponse {
    (
        [(header::CACHE_CONTROL, "no-store")],
        Json(serde_json::json!({
            "name": "CMA Codex provider",
            "version": env!("CARGO_PKG_VERSION"),
            "url": public_base,
            "capabilities": {
                "streaming": true, "activity_items": true, "client_tools": false,
                "cancel": true, "rich_input": false
            },
            "provider_contract": {
                "binding": "/api/v1/cmaeg/bindings/{binding_id}",
                "conversation": "/api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}",
                "responses": "/api/v2/responses"
            },
            "authentication": { "schemes": ["bearer-agent-key"] }
        })),
    )
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
