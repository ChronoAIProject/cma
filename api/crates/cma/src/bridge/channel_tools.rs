//! CMA-authenticated streamable HTTP MCP endpoint for channel-created sessions.
//! Tool calls enqueue independently of Agent turn completion. Browser cookies,
//! NyxID keys and model-supplied routing fields are never accepted as authority.
use std::{sync::Arc, time::Duration};

use axum::{
    extract::{Request, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use layer2_core::bridge::outbox::{
    ChannelReplySessionAuthorization, ChannelReplyToolRequest, OutboxError, ReplyToolReceipt,
};
use layer3_adapter::RuntimeCredential;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::Semaphore;

mod backend;
mod protocol;
pub(crate) use backend::StoredChannelTools;
#[cfg(test)]
mod tests;

const MAX_BODY: usize = 1_100_000;
const VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26"];

#[async_trait::async_trait]
pub(crate) trait ChannelToolBackend: Send + Sync {
    async fn authorize_connection(&self, credential: &RuntimeCredential)
        -> Result<(), OutboxError>;
    async fn authenticate(
        &self,
        credential: &RuntimeCredential,
    ) -> Result<ChannelReplySessionAuthorization, OutboxError>;
    async fn context(
        &self,
        session: &ChannelReplySessionAuthorization,
    ) -> Result<Option<String>, OutboxError>;
    async fn reply(
        &self,
        session: &ChannelReplySessionAuthorization,
        context: &str,
        request: ChannelReplyToolRequest,
    ) -> Result<ReplyToolReceipt, OutboxError>;
}

struct ChannelTools {
    backend: Arc<dyn ChannelToolBackend>,
    public_origin: String,
    readers: Semaphore,
}

pub(crate) fn router(backend: Arc<dyn ChannelToolBackend>, public_origin: &str) -> Router {
    Router::new()
        .route("/api/v1/channel-tools", post(invoke))
        .with_state(Arc::new(ChannelTools {
            backend,
            public_origin: public_origin.trim_end_matches('/').into(),
            readers: Semaphore::new(16),
        }))
}

async fn invoke(State(state): State<Arc<ChannelTools>>, request: Request) -> Response {
    let mut response = match accept(&state, request).await {
        Ok(response) => response,
        Err(status) => (
            status,
            Json(json!({"error":"channel_tool_request_rejected"})),
        )
            .into_response(),
    };
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    response
}

async fn accept(state: &ChannelTools, request: Request) -> Result<Response, StatusCode> {
    let (parts, body) = request.into_parts();
    let credential = headers(&parts.headers, &state.public_origin)?;
    let _slot = state
        .readers
        .try_acquire()
        .map_err(|_| StatusCode::TOO_MANY_REQUESTS)?;
    tokio::time::timeout(
        Duration::from_secs(5),
        state.backend.authorize_connection(&credential),
    )
    .await
    .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
    .map_err(|error| match error {
        OutboxError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        _ => StatusCode::UNAUTHORIZED,
    })?;
    let bytes = tokio::time::timeout(Duration::from_secs(5), axum::body::to_bytes(body, MAX_BODY))
        .await
        .map_err(|_| StatusCode::REQUEST_TIMEOUT)?
        .map_err(|_| StatusCode::PAYLOAD_TOO_LARGE)?;
    let rpc: Rpc = match serde_json::from_slice(&bytes) {
        Ok(rpc) => rpc,
        Err(error) => {
            return Ok(protocol::error(
                Value::Null,
                if error.is_syntax() || error.is_eof() {
                    -32700
                } else {
                    -32600
                },
                "Invalid JSON-RPC request",
            ))
        }
    };
    if rpc.jsonrpc != "2.0"
        || rpc.method.is_empty()
        || rpc.id.as_ref().is_some_and(|id| {
            (!id.is_string() && !id.is_i64() && !id.is_u64() && !id.is_null())
                || id.as_str().is_some_and(|id| id.len() > 256)
        })
    {
        return Ok(protocol::error(
            Value::Null,
            -32600,
            "Invalid JSON-RPC request",
        ));
    }
    let Some(id) = rpc.id else {
        // Notifications cannot create an unacknowledged external send.
        return Ok(StatusCode::ACCEPTED.into_response());
    };
    let session = if rpc.method == "tools/call" {
        Some(
            tokio::time::timeout(
                Duration::from_secs(5),
                state.backend.authenticate(&credential),
            )
            .await
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
            .map_err(|_| StatusCode::UNAUTHORIZED)?,
        )
    } else {
        None
    };
    let result = tokio::time::timeout(
        Duration::from_secs(15),
        protocol::dispatch(
            state.backend.as_ref(),
            session.as_ref(),
            &rpc.method,
            rpc.params,
        ),
    )
    .await;
    Ok(match result {
        Ok(Ok(result)) => Json(json!({"jsonrpc":"2.0", "id":id, "result":result})).into_response(),
        Ok(Err((code, message))) => protocol::error(id, code, message),
        Err(_) => protocol::error(
            id,
            -32000,
            "Operation outcome is unconfirmed; retry with the same request_key and text",
        ),
    })
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

fn single<'a>(headers: &'a HeaderMap, name: &str) -> Result<Option<&'a str>, StatusCode> {
    let mut values = headers.get_all(name).iter();
    let value = values
        .next()
        .map(|value| value.to_str())
        .transpose()
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    if values.next().is_some() {
        return Err(StatusCode::BAD_REQUEST);
    }
    Ok(value)
}

fn headers(headers: &HeaderMap, public_origin: &str) -> Result<RuntimeCredential, StatusCode> {
    if single(headers, "origin")?.is_some_and(|value| value != public_origin) {
        return Err(StatusCode::FORBIDDEN);
    }
    if single(headers, "mcp-protocol-version")?.is_some_and(|value| !VERSIONS.contains(&value)) {
        return Err(StatusCode::BAD_REQUEST);
    }
    if single(headers, "content-type")?
        .and_then(|value| value.split(';').next())
        .is_none_or(|value| !value.trim().eq_ignore_ascii_case("application/json"))
    {
        return Err(StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }
    let raw = single(headers, "authorization")?
        .and_then(|value| value.strip_prefix("Bearer "))
        .ok_or(StatusCode::UNAUTHORIZED)?;
    if !valid_credential(raw) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    Ok(RuntimeCredential::new(raw))
}

pub(crate) fn valid_credential(value: &str) -> bool {
    value.strip_prefix("cma_ct_").is_some_and(|suffix| {
        suffix.len() == 64
            && suffix
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}
