//! Stateless Streamable HTTP for bounded Agent consent and document tools.
use std::{sync::Arc, time::Duration};

use axum::{
    body::to_bytes,
    extract::{Request, State},
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use layer2_core::sandbox::agent_access::AgentAccessError;
use layer2_core::sandbox::agent_access_tool::{
    AgentAccessToolAdmission, AgentAccessToolAuthenticator,
};
use serde::Deserialize;
use serde_json::{json, Value};

mod dispatch;
mod documents;
mod deliverables;
mod limits;
mod reply;
#[cfg(test)]
mod tests;
const VERSION: &str = "2025-06-18";
const BODY_LIMIT: usize = 6 * layer2_core::document_artifacts::MAX_DOCUMENT_BYTES + 16 * 1024;
const DEADLINE: Duration = Duration::from_secs(15);

struct AccessState {
    auth: Arc<dyn AgentAccessToolAuthenticator>,
    admission: Arc<dyn AgentAccessToolAdmission>,
    documents: Option<Arc<layer2_core::document_artifacts::DocumentService>>,
    deliverables: Option<Arc<layer2_core::sandbox::answer_deliverables::AnswerDeliverableService>>,
    public_origin: String,
    concurrency: tokio::sync::Semaphore,
    rate: limits::RateLimit,
}

/// Mount outside human/browser authentication; the Agent bearer is deliberately
/// accepted only by this capability-specific endpoint.
pub fn agent_access_router(
    auth: Arc<dyn AgentAccessToolAuthenticator>,
    admission: Arc<dyn AgentAccessToolAdmission>,
) -> Router {
    agent_tools_router(auth, admission, None, "")
}

/// Runtime authority is confined to consent proposals and private documents
/// authored by this exact Agent. It never conveys owner/share administration.
pub fn agent_tools_router(
    auth: Arc<dyn AgentAccessToolAuthenticator>,
    admission: Arc<dyn AgentAccessToolAdmission>,
    documents: Option<Arc<layer2_core::document_artifacts::DocumentService>>,
    public_origin: &str,
) -> Router {
    agent_delivery_tools_router(auth, admission, documents, None, public_origin)
}

pub fn agent_delivery_tools_router(
    auth: Arc<dyn AgentAccessToolAuthenticator>,
    admission: Arc<dyn AgentAccessToolAdmission>,
    documents: Option<Arc<layer2_core::document_artifacts::DocumentService>>,
    deliverables: Option<Arc<layer2_core::sandbox::answer_deliverables::AnswerDeliverableService>>,
    public_origin: &str,
) -> Router {
    Router::new()
        .route("/api/v1/agent-tools", post(invoke))
        .layer(axum::middleware::from_fn(headers))
        .with_state(Arc::new(AccessState {
            auth,
            admission,
            documents,
            deliverables,
            public_origin: public_origin.trim_end_matches('/').to_owned(),
            concurrency: tokio::sync::Semaphore::new(32),
            rate: limits::RateLimit::default(),
        }))
}

async fn headers(request: Request, next: axum::middleware::Next) -> Response {
    let mut response = next.run(request).await;
    for (name, value) in [
        (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
        (
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ),
        (
            axum::http::HeaderName::from_static("mcp-protocol-version"),
            HeaderValue::from_static(VERSION),
        ),
    ] {
        response.headers_mut().insert(name, value);
    }
    response
}

async fn invoke(State(state): State<Arc<AccessState>>, request: Request) -> Response {
    let Ok(_permit) = state.concurrency.try_acquire() else {
        return busy();
    };
    match tokio::time::timeout(DEADLINE, accept(&state, request)).await {
        Ok(response) => response,
        Err(_) => http_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Agent access service is temporarily unavailable",
        ),
    }
}

async fn accept(state: &AccessState, request: Request) -> Response {
    // This machine-only endpoint has no browser flow or cross-origin grants.
    if request.headers().contains_key(header::ORIGIN) {
        return http_error(StatusCode::FORBIDDEN, "Browser origins are not accepted");
    }
    let mut protocols = request.headers().get_all("mcp-protocol-version").iter();
    let protocol = protocols.next();
    if protocols.next().is_some() || protocol.is_some_and(|value| value != VERSION) {
        return http_error(StatusCode::BAD_REQUEST, "Unsupported MCP protocol version");
    }
    let capability = match capability(request.headers()) {
        Some(value) => zeroize::Zeroizing::new(value.to_owned()),
        None => return unauthorized(),
    };
    let mut types = request.headers().get_all(header::CONTENT_TYPE).iter();
    if !types
        .next()
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<mime::Mime>().ok())
        .is_some_and(|value| value.type_() == mime::APPLICATION && value.subtype() == mime::JSON)
        || types.next().is_some()
    {
        return http_error(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "application/json is required",
        );
    }
    let bytes = match tokio::time::timeout(
        Duration::from_secs(5),
        to_bytes(request.into_body(), BODY_LIMIT),
    )
    .await
    {
        Ok(Ok(bytes)) => bytes,
        Ok(Err(_)) => {
            return http_error(
                StatusCode::PAYLOAD_TOO_LARGE,
                "MCP request body is too large",
            )
        }
        Err(_) => return http_error(StatusCode::REQUEST_TIMEOUT, "MCP request body timed out"),
    };
    let rpc: Rpc = match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(error) => {
            return rpc_error(
                Value::Null,
                if error.is_syntax() || error.is_eof() {
                    -32700
                } else {
                    -32600
                },
                "Invalid JSON-RPC request",
            )
        }
    };
    let document_call = rpc.method == "tools/call"
        && rpc
            .params
            .get("name")
            .and_then(Value::as_str)
            .is_some_and(|name| matches!(name, "publish_document" | "update_document"));
    if !document_call && bytes.len() > 16 * 1024 {
        return http_error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "MCP request body is too large",
        );
    }
    if rpc.jsonrpc != "2.0"
        || rpc.method.is_empty()
        || rpc.method.len() > 128
        || (!rpc.params.is_null() && !rpc.params.is_object())
        || rpc.id.as_ref().is_some_and(|id| {
            (!id.is_string() && !id.is_i64() && !id.is_u64())
                || id
                    .as_str()
                    .is_some_and(|id| id.len() > 128 || id.chars().any(char::is_control))
        })
    {
        return rpc_error(Value::Null, -32600, "Invalid JSON-RPC request");
    }
    dispatch::invoke(state, &capability, rpc).await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Rpc {
    jsonrpc: String,
    #[serde(default, deserialize_with = "rpc_id")]
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Value,
}
fn rpc_id<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Option<Value>, D::Error> {
    Value::deserialize(deserializer).map(Some)
}

fn capability(headers: &axum::http::HeaderMap) -> Option<&str> {
    let mut values = headers.get_all(header::AUTHORIZATION).iter();
    let value = values.next()?.to_str().ok()?.strip_prefix("Bearer ")?;
    if values.next().is_some() {
        return None;
    }
    let suffix = value.strip_prefix("cma_at_")?;
    (suffix.len() == 64
        && suffix
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)))
    .then_some(value)
}

fn rpc_error(id: Value, code: i32, message: &'static str) -> Response {
    Json(json!({"jsonrpc":"2.0", "id":id, "error":{"code":code,"message":message}})).into_response()
}
fn result(id: Value, result: Value) -> Response {
    Json(json!({"jsonrpc":"2.0", "id":id, "result":result})).into_response()
}
fn http_error(status: StatusCode, message: &'static str) -> Response {
    (status, Json(json!({"error":message}))).into_response()
}
fn unauthorized() -> Response {
    let mut response = http_error(
        StatusCode::UNAUTHORIZED,
        "Agent access capability is not valid",
    );
    response
        .headers_mut()
        .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
    response
}
fn auth_error(error: AgentAccessError) -> Response {
    match error {
        AgentAccessError::Unavailable => http_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Agent access service is temporarily unavailable",
        ),
        _ => unauthorized(),
    }
}
fn busy() -> Response {
    let mut response = http_error(
        StatusCode::TOO_MANY_REQUESTS,
        "Agent access request limit reached",
    );
    response
        .headers_mut()
        .insert(header::RETRY_AFTER, HeaderValue::from_static("60"));
    response
}
