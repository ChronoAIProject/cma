//! Public Event Gateway portal transport using the existing CMA browser login.
//! Credentials remain server-held; the Gateway still verifies its own audience
//! and permissions through NyxID on each downstream request.
use super::AuthState;
use axum::{
    body::{to_bytes, Body},
    extract::{Request, State},
    http::{header, HeaderMap, Method, StatusCode},
    response::{IntoResponse, Response},
    routing::any,
    Json, Router,
};
use std::{sync::Arc, time::Duration};

mod credentials;
#[cfg(test)]
mod tests;

const PREFIX: &str = "/api/event-gateway";
const BODY_LIMIT: usize = 1_048_576;

struct Portal {
    auth: Arc<AuthState>,
    base: String,
    origin: String,
    http: reqwest::Client,
}

pub(crate) fn router(
    auth: Arc<AuthState>,
    gateway: Option<&crate::config::GatewayConfig>,
) -> Result<Router, String> {
    let Some(gateway) = gateway else {
        return Ok(Router::new());
    };
    let super::AuthMode::Provider { public_url, .. } = &auth.mode else {
        return Ok(Router::new());
    };
    let origin = reqwest::Url::parse(public_url)
        .map_err(|_| "invalid portal origin")?
        .origin()
        .ascii_serialization();
    let http = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(2))
        .timeout(Duration::from_secs(125))
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .build()
        .map_err(|_| "portal transport unavailable")?;
    Ok(Router::new()
        .route("/api/event-gateway/{*path}", any(forward))
        .with_state(Arc::new(Portal {
            auth,
            base: gateway.base_url.trim_end_matches('/').into(),
            origin,
            http,
        })))
}

fn allowed(method: &Method, path: &str) -> bool {
    let parts = path.split('/').collect::<Vec<_>>();
    let id = |value: &str| {
        !value.is_empty()
            && value.len() <= 8192
            && value
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"._-~".contains(&c))
            && value != "."
            && value != ".."
    };
    match parts.as_slice() {
        ["", "auth", "session"] | ["", "openapi.json"] | ["", "v1", "permissions"] => {
            method == Method::GET
        }
        ["", "v1", "channels"] => matches!(*method, Method::GET | Method::POST),
        ["", "v1", "channels", value] if id(value) => {
            matches!(*method, Method::GET | Method::PUT | Method::DELETE)
        }
        ["", "v1", "channels", value, "test"] if id(value) => method == Method::POST,
        ["", "v1", "channel-records", value] if id(value) => method == Method::GET,
        _ => false,
    }
}

async fn forward(State(portal): State<Arc<Portal>>, request: Request) -> Response {
    match forward_checked(&portal, request).await {
        Ok(response) => response,
        Err(status) => problem(status),
    }
}

async fn forward_checked(portal: &Portal, request: Request) -> Result<Response, StatusCode> {
    let path = request
        .uri()
        .path()
        .strip_prefix(PREFIX)
        .ok_or(StatusCode::NOT_FOUND)?;
    if !allowed(request.method(), path) {
        return Err(StatusCode::NOT_FOUND);
    }
    let mutation = request.method() != Method::GET;
    if mutation
        && (request.headers().get_all(header::ORIGIN).iter().count() != 1
            || request
                .headers()
                .get(header::ORIGIN)
                .and_then(|v| v.to_str().ok())
                != Some(portal.origin.as_str()))
    {
        return Err(StatusCode::FORBIDDEN);
    }
    let custody = credentials::resolve(&portal.auth, request.headers()).await?;
    let target = format!(
        "{}{}",
        portal.base,
        request
            .uri()
            .path_and_query()
            .ok_or(StatusCode::BAD_REQUEST)?
            .as_str()
            .strip_prefix(PREFIX)
            .ok_or(StatusCode::BAD_REQUEST)?
    );
    let mut outgoing = portal
        .http
        .request(request.method().clone(), target)
        .bearer_auth(custody.token());
    // No browser cookies, selected owner, forwarded identity, Host or hop-by-hop
    // headers cross the boundary. Only public API concurrency/content metadata.
    for name in [
        header::CONTENT_TYPE,
        header::ACCEPT,
        header::IF_MATCH,
        header::HeaderName::from_static("idempotency-key"),
    ] {
        if request.headers().get_all(&name).iter().count() > 1 {
            return Err(StatusCode::BAD_REQUEST);
        }
        if let Some(value) = request.headers().get(&name) {
            outgoing = outgoing.header(name, value);
        }
    }
    let body = to_bytes(request.into_body(), BODY_LIMIT)
        .await
        .map_err(|_| StatusCode::PAYLOAD_TOO_LARGE)?;
    if !mutation && !body.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    custody.current(&portal.auth).await?;
    let mut upstream = outgoing
        .body(body)
        .send()
        .await
        .map_err(|_| StatusCode::BAD_GATEWAY)?;
    if upstream.status().is_redirection() {
        return Err(StatusCode::BAD_GATEWAY);
    }
    let status = upstream.status();
    let mut headers = HeaderMap::new();
    for name in [
        header::CONTENT_TYPE,
        header::ETAG,
        header::RETRY_AFTER,
        header::HeaderName::from_static("cmaeg-protocol-version"),
    ] {
        if let Some(value) = upstream.headers().get(&name) {
            headers.insert(name, value.clone());
        }
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = upstream
        .chunk()
        .await
        .map_err(|_| StatusCode::BAD_GATEWAY)?
    {
        if bytes.len().saturating_add(chunk.len()) > BODY_LIMIT {
            return Err(StatusCode::BAD_GATEWAY);
        }
        bytes.extend_from_slice(&chunk);
    }
    custody.current(&portal.auth).await?;
    headers.insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        header::HeaderValue::from_static("nosniff"),
    );
    Ok((status, headers, Body::from(bytes)).into_response())
}

fn problem(status: StatusCode) -> Response {
    (status, [(header::CACHE_CONTROL, "no-store")], Json(serde_json::json!({
        "status": status.as_u16(), "detail": match status {
            StatusCode::UNAUTHORIZED => "Sign in to CMA to open Event Gateway.",
            StatusCode::FORBIDDEN => "This request is not allowed from this origin.",
            StatusCode::NOT_FOUND => "This Event Gateway route is unavailable through the portal.",
            _ => "Event Gateway authorization or transport is temporarily unavailable."
        }
    }))).into_response()
}
