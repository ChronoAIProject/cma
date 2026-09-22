use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse as _, Response};
use axum::routing::any;
use axum::Router;
use layer3_adapter::CredentialFrame;
use tokio::sync::RwLock;

use crate::config::{ProxyConfig, RuntimeConfig};
use crate::failure::{Result, RuntimeError};
use crate::upstream_target::TargetBase;

const MAX_REQUEST_BYTES: usize = 16 * 1024 * 1024;
const MAX_CREDENTIAL_BYTES: usize = 64 * 1024;

#[derive(Clone)]
struct Credential {
    value: String,
    generation: Option<u64>,
}

#[derive(Clone, Default)]
pub(crate) struct CredentialHolder {
    inner: Arc<RwLock<Option<Credential>>>,
    pub(crate) tool_proxy_serving: Arc<std::sync::atomic::AtomicBool>,
}

impl CredentialHolder {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) async fn install(&self, frame: &CredentialFrame) -> Result<()> {
        let value = frame.value.expose();
        if value.is_empty()
            || value.len() > MAX_CREDENTIAL_BYTES
            || value.contains('\r')
            || value.contains('\n')
        {
            return Err(RuntimeError::rejected(
                "credential_invalid",
                "runtime credential is empty or not a usable bearer value",
            ));
        }
        if frame.custody_generation == Some(0) {
            return Err(RuntimeError::rejected(
                "credential_generation_invalid",
                "credential custody generation must be positive",
            ));
        }
        let candidate = Credential {
            value: value.to_string(),
            generation: frame.custody_generation,
        };
        let mut current = self.inner.write().await;
        if let Some(installed) = current.as_ref() {
            match (installed.generation, candidate.generation) {
                (Some(old), Some(new)) if new < old => {
                    return Err(RuntimeError::rejected(
                        "credential_generation_rollback",
                        "credential custody generation moved backwards",
                    ));
                }
                (Some(old), Some(new)) if new == old && installed.value != candidate.value => {
                    return Err(RuntimeError::rejected(
                        "credential_generation_conflict",
                        "credential custody generation was reused with another value",
                    ));
                }
                (Some(_), None) => {
                    return Err(RuntimeError::rejected(
                        "credential_generation_rollback",
                        "a generated credential cannot be replaced without its generation",
                    ));
                }
                _ => {}
            }
        }
        let replaced = current.is_some();
        *current = Some(candidate);
        tracing::info!(replaced, "runtime credential installed");
        Ok(())
    }

    async fn get(&self) -> Option<Credential> {
        self.inner.read().await.clone()
    }

    pub(crate) async fn tool_proxy_generation(&self) -> Option<u64> {
        if !self
            .tool_proxy_serving
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return None;
        }
        self.get().await.and_then(|value| value.generation)
    }

    pub(crate) async fn current(&self) -> Option<layer3_adapter::RuntimeCredential> {
        self.get()
            .await
            .map(|value| layer3_adapter::RuntimeCredential::new(value.value))
    }
}

#[derive(Clone)]
struct ProxyState {
    target: TargetBase,
    credential: CredentialHolder,
    client: reqwest::Client,
    allowed_host: Option<&'static str>,
}

struct PreparedProxy {
    role: &'static str,
    bind: std::net::SocketAddr,
    listener: tokio::net::TcpListener,
    target: TargetBase,
}

pub(crate) struct PreparedProxies {
    proxies: Vec<PreparedProxy>,
    github: Option<crate::github_proxy::PreparedGithubProxy>,
    tools: Option<crate::tool_proxy::PreparedToolProxy>,
}

impl PreparedProxies {
    pub(crate) async fn bind(config: &RuntimeConfig) -> Result<Self> {
        let mut proxies = Vec::new();
        for proxy in [&config.model_proxy, &config.platform_proxy]
            .into_iter()
            .flatten()
        {
            proxies.push(bind_proxy(proxy).await?);
        }
        let github = match &config.github_proxy {
            Some(config) => Some(crate::github_proxy::PreparedGithubProxy::bind(config).await?),
            None => None,
        };
        let tools = match &config.tool_proxy {
            Some(config) => Some(crate::tool_proxy::PreparedToolProxy::bind(config).await?),
            None => None,
        };
        Ok(Self {
            proxies,
            github,
            tools,
        })
    }

    pub(crate) fn start(self, credential: CredentialHolder) {
        if let Some(tools) = self.tools {
            tools.start(credential.clone());
        }
        if let Some(github) = self.github {
            github.start(credential.clone());
        }
        for proxy in self.proxies {
            spawn_proxy(proxy, credential.clone());
        }
    }
}

async fn bind_proxy(config: &ProxyConfig) -> Result<PreparedProxy> {
    let target = TargetBase::parse(&config.upstream)?;
    let listener = tokio::net::TcpListener::bind(config.bind)
        .await
        .map_err(|error| {
            RuntimeError::unavailable(
                "proxy_bind_failed",
                format!(
                    "bind {} credential proxy at {}: {error}",
                    config.role, config.bind
                ),
            )
        })?;
    Ok(PreparedProxy {
        role: config.role,
        bind: config.bind,
        listener,
        target,
    })
}

fn spawn_proxy(proxy: PreparedProxy, credential: CredentialHolder) {
    let app = router(proxy.target, credential, None);
    tokio::spawn(async move {
        tracing::info!(proxy = proxy.role, bind = %proxy.bind, "credential proxy listening");
        if let Err(error) = axum::serve(proxy.listener, app).await {
            tracing::error!(proxy = proxy.role, %error, "credential proxy stopped");
        }
    });
}

pub(crate) fn router(
    target: TargetBase,
    credential: CredentialHolder,
    allowed_host: Option<&'static str>,
) -> Router {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(600))
        .build()
        .expect("credential proxy HTTP client builds");
    let state = ProxyState {
        target,
        credential,
        client,
        allowed_host,
    };
    Router::new().fallback(any(forward)).with_state(state)
}

async fn forward(State(state): State<ProxyState>, request: Request) -> Response {
    if state
        .allowed_host
        .is_some_and(|host| !allowed_request_host(&request, host))
    {
        return (StatusCode::FORBIDDEN, "proxy destination is not permitted").into_response();
    }
    let Some(credential) = state.credential.get().await else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "no upstream credential available yet",
        )
            .into_response();
    };
    let (parts, body) = request.into_parts();
    let target = match state.target.join(&parts.uri) {
        Ok(target) => target,
        Err(error) => return (StatusCode::BAD_REQUEST, error.detail).into_response(),
    };
    let body = match axum::body::to_bytes(body, MAX_REQUEST_BYTES).await {
        Ok(body) => body,
        Err(_) => {
            return (
                StatusCode::PAYLOAD_TOO_LARGE,
                "proxy request body is too large",
            )
                .into_response()
        }
    };
    let mut outbound = reqwest::Request::new(parts.method, target);
    copy_request_headers(&parts.headers, outbound.headers_mut());
    let bearer = match HeaderValue::from_str(&format!("Bearer {}", credential.value)) {
        Ok(value) => value,
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "runtime credential is not a usable header",
            )
                .into_response()
        }
    };
    outbound.headers_mut().insert(header::AUTHORIZATION, bearer);
    *outbound.body_mut() = Some(body.to_vec().into());
    let response = match state.client.execute(outbound).await {
        Ok(response) => response,
        Err(error) => {
            tracing::warn!(%error, "credential proxy upstream request failed");
            return (StatusCode::BAD_GATEWAY, "upstream request failed").into_response();
        }
    };
    if encoded(response.headers())
        || headers_contain(response.headers(), credential.value.as_bytes())
    {
        return (
            StatusCode::BAD_GATEWAY,
            "upstream response was refused at the credential boundary",
        )
            .into_response();
    }
    let status = response.status();
    let headers = response.headers().clone();
    let stream =
        crate::upstream_guard::guarded(response.bytes_stream(), credential.value.into_bytes());
    let mut proxied = Response::new(Body::from_stream(stream));
    *proxied.status_mut() = status;
    copy_response_headers(&headers, proxied.headers_mut());
    proxied
}

fn allowed_request_host(request: &Request, host: &str) -> bool {
    let matches = |value: &str| {
        value.eq_ignore_ascii_case(host) || value.eq_ignore_ascii_case(&format!("{host}:443"))
    };
    let mut hosts = request.headers().get_all(header::HOST).iter();
    let valid_host = hosts
        .next()
        .and_then(|value| value.to_str().ok())
        .is_some_and(matches)
        && hosts.next().is_none();
    valid_host
        && request.method() != axum::http::Method::CONNECT
        && request
            .uri()
            .scheme_str()
            .is_none_or(|scheme| scheme == "https")
        && request
            .uri()
            .authority()
            .is_none_or(|authority| matches(authority.as_str()))
}

pub(crate) fn copy_request_headers(source: &HeaderMap, target: &mut HeaderMap) {
    for (name, value) in source {
        if !owned_header(name.as_str()) {
            target.insert(name, value.clone());
        }
    }
    target.insert(
        header::ACCEPT_ENCODING,
        HeaderValue::from_static("identity"),
    );
}

pub(crate) fn copy_response_headers(source: &HeaderMap, target: &mut HeaderMap) {
    for (name, value) in source {
        if !hop_header(name.as_str()) && name != header::CONTENT_LENGTH {
            target.insert(name, value.clone());
        }
    }
}

fn owned_header(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "authorization"
            | "open-sandbox-api-key"
            | "host"
            | "content-length"
            | "accept-encoding"
            | "connection"
            | "proxy-authorization"
            | "proxy-authenticate"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
    )
}

fn hop_header(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
    )
}

pub(crate) fn encoded(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_ENCODING)
        .is_some_and(|value| value.as_bytes() != b"identity")
}

pub(crate) fn headers_contain(headers: &HeaderMap, credential: &[u8]) -> bool {
    headers.iter().any(|(name, value)| {
        crate::upstream_guard::contains(name.as_str().as_bytes(), credential)
            || crate::upstream_guard::contains(value.as_bytes(), credential)
    })
}

#[cfg(test)]
#[path = "upstream_tests.rs"]
mod tests;
