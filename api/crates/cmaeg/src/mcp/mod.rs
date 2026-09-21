//! Stateless MCP streamable HTTP using JSON responses and the shared tools service.
pub mod tools;
use crate::{
    auth::AgentCaller,
    http::problems::{problem, protocol_headers},
    tools::ToolsState,
};
use axum::{
    body::to_bytes,
    extract::{Request, State},
    http::{HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use cmaeg_core::problem::{Problem, ProblemCode};
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Duration;
const VERSION: &str = "2025-06-18";
pub fn router(state: ToolsState) -> Router {
    Router::new()
        .route("/mcp", post(invoke))
        .with_state(state)
        .layer(axum::middleware::from_fn(protocol_headers))
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
async fn invoke(
    State(state): State<ToolsState>,
    caller: AgentCaller,
    request: Request,
) -> Response {
    let mut response = accept(&state, caller, request).await;
    response
        .headers_mut()
        .insert("mcp-protocol-version", HeaderValue::from_static(VERSION));
    response
}
async fn accept(state: &ToolsState, caller: AgentCaller, request: Request) -> Response {
    for (name, expected) in [
        ("origin", state.public_origin.as_str()),
        ("mcp-protocol-version", VERSION),
    ] {
        let mut values = request.headers().get_all(name).iter();
        let value = values.next();
        if values.next().is_some()
            || value.is_some_and(|value| value.to_str().ok() != Some(expected))
        {
            return problem(
                if name == "origin" {
                    StatusCode::FORBIDDEN
                } else {
                    StatusCode::BAD_REQUEST
                },
                ProblemCode::InvalidRequest,
                "unsupported MCP request headers",
            );
        }
    }
    let mut types = request.headers().get_all("content-type").iter();
    if !types
        .next()
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("application/json"))
        || types.next().is_some()
    {
        return problem(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            ProblemCode::UnsupportedMediaType,
            "application/json is required",
        );
    }
    let body = match tokio::time::timeout(
        Duration::from_secs(5),
        to_bytes(request.into_body(), 131072),
    )
    .await
    {
        Ok(Ok(body)) => body,
        Ok(Err(_)) => {
            return problem(
                StatusCode::PAYLOAD_TOO_LARGE,
                ProblemCode::PayloadTooLarge,
                "MCP body exceeds limit",
            )
        }
        Err(_) => {
            return problem(
                StatusCode::REQUEST_TIMEOUT,
                ProblemCode::InvalidRequest,
                "body_timeout",
            )
        }
    };
    let rpc: Rpc = match serde_json::from_slice(&body) {
        Ok(rpc) => rpc,
        Err(parse_error) => {
            return error(
                Value::Null,
                if parse_error.is_syntax() || parse_error.is_eof() {
                    -32700
                } else {
                    -32600
                },
                "Invalid JSON-RPC request",
                None,
            )
        }
    };
    if rpc.jsonrpc != "2.0"
        || rpc.method.is_empty()
        || rpc.method.len() > 256
        || rpc.id.as_ref().is_some_and(|id| {
            (!id.is_string() && !id.is_i64() && !id.is_u64() && !id.is_null())
                || id.as_str().is_some_and(|id| id.len() > 256)
        })
    {
        return error(Value::Null, -32600, "Invalid JSON-RPC request", None);
    }
    let Some(id) = rpc.id else {
        return StatusCode::ACCEPTED.into_response();
    };
    let result = match rpc.method.as_str() {
        "initialize" => {
            if !rpc
                .params
                .get("protocolVersion")
                .is_some_and(Value::is_string)
            {
                return error(id, -32602, "protocolVersion is required", None);
            }
            Ok(
                json!({"protocolVersion":VERSION,"serverInfo":{"name":"cmaeg","version":"1"},"capabilities":{"tools":{"listChanged":false}}}),
            )
        }
        "ping" => Ok(json!({})),
        "tools/list" => Ok(tools::list()),
        "tools/call" => {
            let call: Call = match serde_json::from_value(rpc.params) {
                Ok(call) => call,
                Err(_) => return error(id, -32602, "Invalid tool arguments", None),
            };
            tools::call(state, caller, &call.name, call.arguments).await
        }
        _ => return error(id, -32601, "Method not found", None),
    };
    match result {
        Ok(value) => Json(json!({"jsonrpc":"2.0","id":id,"result":value})).into_response(),
        Err(problem) => error(id, -32000, "Tool operation refused", Some(problem)),
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Call {
    name: String,
    arguments: Value,
    #[serde(default, rename = "_meta")]
    _meta: Option<Value>,
}
fn error(id: Value, code: i32, message: &str, data: Option<Problem>) -> Response {
    let mut error = json!({"code":code,"message":message});
    if let Some(data) = data {
        error["data"] = serde_json::to_value(data).unwrap();
    }
    Json(json!({"jsonrpc":"2.0","id":id,"error":error})).into_response()
}
