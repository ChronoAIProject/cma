//! One fixed external tool endpoint using the runtime's current credential.
use std::{collections::BTreeMap, net::SocketAddr, time::Duration};

use axum::{
    body::{to_bytes, Body},
    extract::{Request, State},
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Router,
};

use crate::{
    failure::{Result, RuntimeError},
    upstream::CredentialHolder,
};

const MAX_REQUEST_BYTES: usize = 128 * 1024;
// Full context can occupy 1 MiB before a tool result carries both structured
// JSON and its escaped text encoding. Retain a finite bound for that envelope.
const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
const BODY_DEADLINE: Duration = Duration::from_secs(5);
// Native upload tools can await the gateway for 65 s. Keep a bounded response
// margin without inspecting protocol payloads or changing gateway send leases.
const UPSTREAM_DEADLINE: Duration = Duration::from_secs(70);

#[derive(Clone, Debug)]
pub(crate) struct ToolProxyConfig {
    pub upstream: url::Url,
}

impl ToolProxyConfig {
    pub(crate) fn from_env(
        env: &BTreeMap<String, String>,
    ) -> std::result::Result<Option<Self>, String> {
        let Some(value) = env
            .get("CHRONO_TOOL_PROXY_URL")
            .filter(|value| !value.is_empty())
        else {
            return Ok(None);
        };
        let invalid =
            || "CHRONO_TOOL_PROXY_URL must name one credential-free HTTPS endpoint".to_string();
        let upstream = url::Url::parse(value).map_err(|_| invalid())?;
        let loopback = upstream.host_str().is_some_and(|host| {
            host.trim_matches(['[', ']'])
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
        });
        if upstream.as_str() != value
            || upstream.host_str().is_none()
            || !(upstream.scheme() == "https" || (upstream.scheme() == "http" && loopback))
            || !upstream.username().is_empty()
            || upstream.password().is_some()
            || upstream.port() == Some(0)
            || upstream.query().is_some()
            || upstream.fragment().is_some()
            || crate::upstream_target::unsafe_path(upstream.path())
        {
            return Err(invalid());
        }
        Ok(Some(Self { upstream }))
    }
}

pub(crate) struct PreparedToolProxy {
    listener: tokio::net::TcpListener,
    config: ToolProxyConfig,
}

impl PreparedToolProxy {
    pub(crate) async fn bind(config: &ToolProxyConfig) -> Result<Self> {
        let bind: SocketAddr = "127.0.0.1:9093".parse().expect("literal loopback address");
        let listener = tokio::net::TcpListener::bind(bind).await.map_err(|_| {
            RuntimeError::unavailable(
                "tool_proxy_bind_failed",
                "could not bind local tool endpoint",
            )
        })?;
        Ok(Self {
            listener,
            config: config.clone(),
        })
    }

    pub(crate) fn start(self, credential: CredentialHolder) {
        let serving = Serving(credential.tool_proxy_serving.clone());
        serving.0.store(true, std::sync::atomic::Ordering::Release);
        let app = router(self.config, credential);
        tokio::spawn(async move {
            let _serving = serving;
            if axum::serve(self.listener, app).await.is_err() {
                tracing::error!("local tool endpoint stopped");
            }
        });
    }
}

struct Serving(std::sync::Arc<std::sync::atomic::AtomicBool>);

impl Drop for Serving {
    fn drop(&mut self) {
        self.0.store(false, std::sync::atomic::Ordering::Release);
    }
}

#[derive(Clone)]
struct ProxyState {
    config: ToolProxyConfig,
    credential: CredentialHolder,
    client: reqwest::Client,
}

fn router(config: ToolProxyConfig, credential: CredentialHolder) -> Router {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(2))
        .timeout(UPSTREAM_DEADLINE)
        .build()
        .expect("tool proxy HTTP client builds");
    Router::new()
        .route("/", post(forward))
        .with_state(ProxyState {
            config,
            credential,
            client,
        })
}

async fn forward(State(state): State<ProxyState>, request: Request) -> Response {
    // The request URI and headers cannot choose a different remote destination.
    if request.uri().query().is_some()
        || request.uri().scheme().is_some()
        || request.uri().authority().is_some()
    {
        return (
            StatusCode::BAD_REQUEST,
            "tool endpoint does not accept a destination",
        )
            .into_response();
    }
    let (parts, body) = request.into_parts();
    let body = match tokio::time::timeout(BODY_DEADLINE, to_bytes(body, MAX_REQUEST_BYTES)).await {
        Ok(Ok(body)) => body,
        Ok(Err(_)) => {
            return (StatusCode::PAYLOAD_TOO_LARGE, "tool request is too large").into_response()
        }
        Err(_) => {
            return (StatusCode::REQUEST_TIMEOUT, "tool request body timed out").into_response()
        }
    };
    let Some(credential) = state.credential.current().await else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "tool credential is not available",
        )
            .into_response();
    };
    let mut outbound = reqwest::Request::new(reqwest::Method::POST, state.config.upstream.clone());
    crate::upstream::copy_request_headers(&parts.headers, outbound.headers_mut());
    // An origin supplied to the local endpoint does not describe the remote origin.
    outbound.headers_mut().remove(header::ORIGIN);
    let Ok(mut bearer) = HeaderValue::from_str(&format!("Bearer {}", credential.expose())) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "tool credential is not usable",
        )
            .into_response();
    };
    bearer.set_sensitive(true);
    outbound.headers_mut().insert(header::AUTHORIZATION, bearer);
    *outbound.body_mut() = Some(body.to_vec().into());
    let Ok(mut remote) = state.client.execute(outbound).await else {
        return (
            StatusCode::BAD_GATEWAY,
            "tool request outcome is unconfirmed",
        )
            .into_response();
    };
    if remote.status().is_redirection()
        || crate::upstream::encoded(remote.headers())
        || crate::upstream::headers_contain(remote.headers(), credential.expose().as_bytes())
    {
        return refused();
    }
    let mut bytes = Vec::new();
    loop {
        match remote.chunk().await {
            Ok(Some(chunk)) if chunk.len() <= MAX_RESPONSE_BYTES.saturating_sub(bytes.len()) => {
                bytes.extend_from_slice(&chunk)
            }
            Ok(None) => break,
            _ => return refused(),
        }
    }
    if crate::upstream_guard::contains(&bytes, credential.expose().as_bytes()) {
        return refused();
    }
    let mut response = Response::new(Body::from(bytes));
    *response.status_mut() = remote.status();
    crate::upstream::copy_response_headers(remote.headers(), response.headers_mut());
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn refused() -> Response {
    (
        StatusCode::BAD_GATEWAY,
        "tool response was refused; outcome is unconfirmed",
    )
        .into_response()
}

#[cfg(test)]
#[path = "tool_proxy_tests.rs"]
mod tests;
