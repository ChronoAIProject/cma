//! Fixed canonical MCP transport with server-held issuer custody.
use crate::{auth::persistence::SecretValue, store::MongoProviderToolSource};
use axum::{
    body::{to_bytes, Body},
    extract::{Request, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Router,
};
use std::{sync::Arc, time::Duration};

const REQUEST_BYTES: usize = 128 * 1024;
const RESPONSE_BYTES: usize = 4 * 1024 * 1024;
const DEADLINE: Duration = Duration::from_secs(70);

#[async_trait::async_trait]
trait Authenticate: Send + Sync {
    async fn current(
        &self,
        capability: &str,
        setup: bool,
    ) -> layer3_adapter::Result<(url::Url, SecretValue)>;
}

#[async_trait::async_trait]
impl Authenticate for MongoProviderToolSource {
    async fn current(
        &self,
        capability: &str,
        setup: bool,
    ) -> layer3_adapter::Result<(url::Url, SecretValue)> {
        let authority = self.authenticate_relay(capability, setup).await?;
        Ok((authority.upstream, authority.credential))
    }
}

#[derive(Clone)]
struct RelayState {
    source: Arc<dyn Authenticate>,
    client: reqwest::Client,
}

pub(crate) fn router(source: Arc<MongoProviderToolSource>) -> Router {
    routes(source)
}

fn routes(source: Arc<dyn Authenticate>) -> Router {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(2))
        .timeout(DEADLINE)
        .no_proxy()
        .no_gzip()
        .no_brotli()
        .no_deflate()
        .no_zstd()
        .build()
        .expect("relay client builds");
    Router::new()
        .route("/api/v1/provider-tools", post(invoke))
        .with_state(RelayState { source, client })
}

async fn invoke(State(state): State<RelayState>, request: Request) -> Response {
    let mut response = match tokio::time::timeout(DEADLINE, accept(state, request)).await {
        Ok(response) => response,
        Err(_) => refused(),
    };
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

async fn accept(state: RelayState, request: Request) -> Response {
    if request.uri().query().is_some()
        || request.uri().scheme().is_some()
        || request.uri().authority().is_some()
        || request.headers().contains_key(header::CONTENT_ENCODING)
    {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let mut authorizations = request.headers().get_all(header::AUTHORIZATION).iter();
    let Some(capability) = authorizations
        .next()
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .filter(|v| v.len() == 136 && !v.chars().any(char::is_whitespace))
        .map(str::to_owned)
    else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    if authorizations.next().is_some() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let mut content_types = request.headers().get_all(header::CONTENT_TYPE).iter();
    if content_types
        .next()
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .is_none_or(|v| !v.trim().eq_ignore_ascii_case("application/json"))
        || content_types.next().is_some()
    {
        return StatusCode::UNSUPPORTED_MEDIA_TYPE.into_response();
    }
    let (parts, body) = request.into_parts();
    let body =
        match tokio::time::timeout(Duration::from_secs(5), to_bytes(body, REQUEST_BYTES)).await {
            Ok(Ok(body)) => body,
            Ok(Err(_)) => return StatusCode::PAYLOAD_TOO_LARGE.into_response(),
            Err(_) => return StatusCode::REQUEST_TIMEOUT.into_response(),
        };
    let Some(setup) = setup_request(&body) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let (upstream, credential) = match state.source.current(&capability, setup).await {
        Ok(authority) => authority,
        Err(layer3_adapter::AdapterError::Unavailable(_)) => {
            return StatusCode::SERVICE_UNAVAILABLE.into_response()
        }
        Err(_) => return StatusCode::UNAUTHORIZED.into_response(),
    };
    forward(
        &state.client,
        upstream,
        credential,
        &parts.headers,
        body.to_vec(),
    )
    .await
}

fn setup_request(body: &[u8]) -> Option<bool> {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Rpc {
        jsonrpc: String,
        method: String,
        #[serde(default)]
        id: Option<serde_json::Value>,
        #[serde(default)]
        params: serde_json::Value,
    }
    let rpc: Rpc = serde_json::from_slice(body).ok()?;
    if rpc.jsonrpc != "2.0" {
        return None;
    }
    let _ = (rpc.id, rpc.params);
    match rpc.method.as_str() {
        "initialize" | "ping" | "tools/list" | "notifications/initialized" => Some(true),
        "tools/call" => Some(false),
        _ => None,
    }
}

async fn forward(
    client: &reqwest::Client,
    upstream: url::Url,
    credential: SecretValue,
    headers: &HeaderMap,
    body: Vec<u8>,
) -> Response {
    let mut outbound = reqwest::Request::new(reqwest::Method::POST, upstream);
    for name in ["content-type", "accept", "mcp-protocol-version"] {
        let mut values = headers.get_all(name).iter();
        if let Some(value) = values.next() {
            if values.next().is_some() {
                return StatusCode::BAD_REQUEST.into_response();
            }
            outbound
                .headers_mut()
                .insert(axum::http::HeaderName::from_static(name), value.clone());
        }
    }
    let Ok(mut bearer) = HeaderValue::from_str(&format!("Bearer {}", credential.expose())) else {
        return refused();
    };
    bearer.set_sensitive(true);
    outbound.headers_mut().insert(header::AUTHORIZATION, bearer);
    outbound.headers_mut().insert(
        header::ACCEPT_ENCODING,
        HeaderValue::from_static("identity"),
    );
    *outbound.body_mut() = Some(body.into());
    // No retry: tools may have effects even when the remote response is lost.
    let Ok(mut remote) = client.execute(outbound).await else {
        return refused();
    };
    if remote.status().is_redirection()
        || remote
            .headers()
            .get(header::CONTENT_ENCODING)
            .is_some_and(|v| v.as_bytes() != b"identity")
        || remote.headers().iter().any(|(name, value)| {
            contains(name.as_str().as_bytes(), credential.expose().as_bytes())
                || contains(value.as_bytes(), credential.expose().as_bytes())
        })
    {
        return refused();
    }
    let mut bytes = Vec::new();
    loop {
        match remote.chunk().await {
            Ok(Some(chunk)) if chunk.len() <= RESPONSE_BYTES.saturating_sub(bytes.len()) => {
                bytes.extend_from_slice(&chunk)
            }
            Ok(None) => break,
            _ => return refused(),
        }
    }
    if contains(&bytes, credential.expose().as_bytes()) {
        return refused();
    }
    if !bytes.is_empty() {
        let Ok(json) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            return refused();
        };
        if reflects(&json, credential.expose()) {
            return refused();
        }
    }
    let mut response = Response::new(Body::from(bytes));
    *response.status_mut() = remote.status();
    for name in ["content-type", "mcp-protocol-version", "retry-after"] {
        if let Some(value) = remote.headers().get(name) {
            response
                .headers_mut()
                .insert(axum::http::HeaderName::from_static(name), value.clone());
        }
    }
    response
}

fn contains(bytes: &[u8], secret: &[u8]) -> bool {
    !secret.is_empty() && bytes.windows(secret.len()).any(|part| part == secret)
}
fn reflects(value: &serde_json::Value, secret: &str) -> bool {
    match value {
        serde_json::Value::String(value) => value.contains(secret),
        serde_json::Value::Array(values) => values.iter().any(|value| reflects(value, secret)),
        serde_json::Value::Object(values) => values
            .iter()
            .any(|(key, value)| key.contains(secret) || reflects(value, secret)),
        _ => false,
    }
}
fn refused() -> Response {
    (StatusCode::BAD_GATEWAY, "tool response is unconfirmed").into_response()
}

#[cfg(test)]
mod tests;
