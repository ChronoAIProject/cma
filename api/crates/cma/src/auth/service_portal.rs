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
mod streaming;
#[cfg(test)]
mod tests;

const PREFIX: &str = "/api/event-gateway";
const BODY_LIMIT: usize = 1_048_576;

struct Portal {
    auth: Arc<AuthState>,
    base: String,
    http: reqwest::Client,
}

#[derive(Debug, thiserror::Error)]
enum PortalError {
    #[error("activity stream authorization changed: {0}")]
    Status(StatusCode),
    #[error("activity stream authorization changed: {0}")]
    Unauthorized(credentials::Refusal),
    #[error("account_deletion_in_progress")]
    AccountDeletionInProgress,
}

impl From<StatusCode> for PortalError {
    fn from(status: StatusCode) -> Self {
        Self::Status(status)
    }
}

impl From<credentials::Refusal> for PortalError {
    fn from(refusal: credentials::Refusal) -> Self {
        Self::Unauthorized(refusal)
    }
}

impl PortalError {
    fn response(self) -> Response {
        match self {
            Self::Status(status) => problem(status, None),
            Self::Unauthorized(refusal) => {
                tracing::info!(
                    reason = refusal.code(),
                    "Event Gateway portal refused the browser credential"
                );
                problem(StatusCode::UNAUTHORIZED, Some(refusal.code()))
            }
            Self::AccountDeletionInProgress => super::account_standing::deletion_problem(),
        }
    }
}

pub(crate) fn router(
    auth: Arc<AuthState>,
    gateway: Option<&crate::config::GatewayConfig>,
) -> Result<Router, String> {
    let Some(gateway) = gateway else {
        return Ok(Router::new());
    };
    if !matches!(auth.mode, super::AuthMode::Provider { .. }) {
        return Ok(Router::new());
    }
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
        ["", "v1", "channel-capabilities"]
        | ["", "v1", "channel-connections"]
        | ["", "v1", "relay-resource-connections"] => method == Method::GET,
        ["", "v1", "channels", value] if id(value) => {
            matches!(*method, Method::GET | Method::PUT | Method::DELETE)
        }
        ["", "v1", "channels", value, "test"] if id(value) => method == Method::POST,
        ["", "v1", "channels", value, "sources", index, "test"]
            if id(value)
                && index
                    .parse::<usize>()
                    .is_ok_and(|parsed| parsed.to_string() == *index) =>
        {
            method == Method::POST
        }
        ["", "v1", "channels", value, "routing"] if id(value) => method == Method::PUT,
        ["", "v1", "channels", value, "outbound-messages"] if id(value) => method == Method::POST,
        ["", "v1", "channels", value, "outbound-messages", delivery]
            if id(value) && id(delivery) =>
        {
            matches!(*method, Method::GET | Method::DELETE)
        }
        ["", "v1", "channels", value, "outbound-messages", delivery, "send"]
            if id(value) && id(delivery) =>
        {
            method == Method::POST
        }
        ["", "v1", "channels", value, "activity"]
        | ["", "v1", "channels", value, "activity", "stream"]
            if id(value) =>
        {
            method == Method::GET
        }
        ["", "v1", "channel-records", value] if id(value) => method == Method::GET,
        _ => false,
    }
}

async fn forward(State(portal): State<Arc<Portal>>, request: Request) -> Response {
    match forward_checked(&portal, request).await {
        Ok(response) => response,
        Err(error) => error.response(),
    }
}

async fn forward_checked(portal: &Portal, request: Request) -> Result<Response, PortalError> {
    let path = request
        .uri()
        .path()
        .strip_prefix(PREFIX)
        .ok_or(StatusCode::NOT_FOUND)?;
    if !allowed(request.method(), path) {
        return Err(StatusCode::NOT_FOUND.into());
    }
    let activity_stream = path.ends_with("/activity/stream");
    let mutation = request.method() != Method::GET;
    if mutation && !portal.auth.permits_browser_origin(request.headers()) {
        return Err(StatusCode::FORBIDDEN.into());
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
            return Err(StatusCode::BAD_REQUEST.into());
        }
        if let Some(value) = request.headers().get(&name) {
            outgoing = outgoing.header(name, value);
        }
    }
    if activity_stream {
        let cursor = header::HeaderName::from_static("last-event-id");
        if request.headers().get_all(&cursor).iter().count() > 1 {
            return Err(StatusCode::BAD_REQUEST.into());
        }
        if let Some(value) = request.headers().get(&cursor) {
            if value.len() > 1024 || value.to_str().is_err() {
                return Err(StatusCode::BAD_REQUEST.into());
            }
            outgoing = outgoing.header(cursor, value);
        }
    }
    let body = to_bytes(request.into_body(), BODY_LIMIT)
        .await
        .map_err(|_| StatusCode::PAYLOAD_TOO_LARGE)?;
    if !mutation && !body.is_empty() {
        return Err(StatusCode::BAD_REQUEST.into());
    }
    custody.current(&portal.auth).await?;
    let mut upstream = outgoing
        .body(body)
        .send()
        .await
        .map_err(|_| StatusCode::BAD_GATEWAY)?;
    if upstream.status().is_redirection() {
        return Err(StatusCode::BAD_GATEWAY.into());
    }
    let status = upstream.status();
    if status == StatusCode::UNAUTHORIZED {
        // The Gateway checks its own audience and permissions; its answer
        // passes through unchanged. Distinguish it from portal refusals.
        tracing::info!(
            reason = "gateway_refused_credential",
            "Event Gateway refused the forwarded browser credential"
        );
    }
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
    if activity_stream && status == StatusCode::OK {
        return streaming::response(upstream, headers, portal.auth.clone(), custody).await;
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = upstream
        .chunk()
        .await
        .map_err(|_| StatusCode::BAD_GATEWAY)?
    {
        if bytes.len().saturating_add(chunk.len()) > BODY_LIMIT {
            return Err(StatusCode::BAD_GATEWAY.into());
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

fn problem(status: StatusCode, code: Option<&'static str>) -> Response {
    let mut body = serde_json::json!({
        "status": status.as_u16(), "detail": match status {
            StatusCode::UNAUTHORIZED => "Sign in to CMA to open Event Gateway.",
            StatusCode::FORBIDDEN => "This request is not allowed from this origin.",
            StatusCode::NOT_FOUND => "This Event Gateway route is unavailable through the portal.",
            _ => "Event Gateway authorization or transport is temporarily unavailable."
        }
    });
    if let Some(code) = code {
        body["code"] = code.into();
    }
    (status, [(header::CACHE_CONTROL, "no-store")], Json(body)).into_response()
}
