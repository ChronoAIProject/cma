use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse as _, Response};
use axum::routing::any;
use axum::Router;
use layer3_adapter::{CredentialFrame, GithubCredentialRoute};
use tokio::sync::RwLock;

use crate::config::{ProxyConfig, RuntimeConfig};
use crate::failure::{Result, RuntimeError};
use crate::upstream_target::TargetBase;
#[path = "github_scope_proxy.rs"]
pub(crate) mod github_scope;
#[path = "github_write_proxy.rs"]
mod github_write;
#[path = "upstream_headers.rs"]
mod headers;
pub(crate) use headers::{copy_request_headers, copy_response_headers, encoded, headers_contain};

const MAX_REQUEST_BYTES: usize = 16 * 1024 * 1024;
const MAX_CREDENTIAL_BYTES: usize = 64 * 1024;

#[derive(Clone, PartialEq, Eq)]
struct Credential {
    value: String,
    generation: Option<u64>,
    github_route: Option<GithubCredentialRoute>,
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
        if frame
            .github_route
            .as_ref()
            .is_some_and(|route| frame.custody_generation.is_none() || !route.is_valid())
        {
            return Err(RuntimeError::rejected(
                "credential_github_route_invalid",
                "GitHub route requires a valid selector and credential generation",
            ));
        }
        let candidate = Credential {
            value: value.to_string(),
            generation: frame.custody_generation,
            github_route: frame.github_route.clone(),
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
                (Some(old), Some(new)) if new == old && installed != &candidate => {
                    return Err(RuntimeError::rejected(
                        "credential_generation_conflict",
                        "credential custody generation was reused with another value or route",
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

    pub(crate) async fn has_github_repository_scope(&self) -> bool {
        self.inner.read().await.as_ref().is_some_and(|credential| {
            credential
                .github_route
                .as_ref()
                .is_some_and(|route| route.repository_scope.is_some())
        })
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
    github_client: Option<reqwest::Client>,
    allowed_host: Option<&'static str>,
    model_proxy: bool,
    model_credit_required: bool,
}

struct PreparedProxy {
    role: &'static str,
    bind: std::net::SocketAddr,
    listener: tokio::net::TcpListener,
    target: TargetBase,
    model_credit_required: bool,
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
            proxies.push(
                bind_proxy(proxy, config.model_credit_required && proxy.role == "model").await?,
            );
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

async fn bind_proxy(config: &ProxyConfig, model_credit_required: bool) -> Result<PreparedProxy> {
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
        model_credit_required,
    })
}

fn spawn_proxy(proxy: PreparedProxy, credential: CredentialHolder) {
    let app = proxy_router(
        proxy.target,
        credential,
        None,
        proxy.role == "model",
        proxy.model_credit_required,
    );
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
    proxy_router(target, credential, allowed_host, false, false)
}

fn proxy_router(
    target: TargetBase,
    credential: CredentialHolder,
    allowed_host: Option<&'static str>,
    model_proxy: bool,
    model_credit_required: bool,
) -> Router {
    let client = proxy_client(reqwest::Client::builder());
    let state = ProxyState {
        target,
        credential,
        client,
        github_client: (!model_proxy).then(github_write::client),
        allowed_host,
        model_proxy,
        model_credit_required,
    };
    Router::new().fallback(any(forward)).with_state(state)
}

fn proxy_client(builder: reqwest::ClientBuilder) -> reqwest::Client {
    builder
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(600))
        .build()
        .expect("credential proxy HTTP client builds")
}

async fn forward(State(state): State<ProxyState>, request: Request) -> Response {
    if state
        .allowed_host
        .is_some_and(|host| !allowed_request_host(&request, host))
    {
        return (StatusCode::FORBIDDEN, "proxy destination is not permitted").into_response();
    }
    if state.model_credit_required {
        // The deployed model adapter cannot enforce an authoritative reservation.
        return (StatusCode::SERVICE_UNAVAILABLE, "model_funding_unsupported").into_response();
    }
    let Some(credential) = state.credential.get().await else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "no upstream credential available yet",
        )
            .into_response();
    };
    let (parts, body) = request.into_parts();
    let route = credential.github_route.as_ref();
    let github_client = state.github_client.as_ref().filter(|_| {
        github_write::is_write(
            state.allowed_host,
            state.model_proxy,
            &parts.method,
            &parts.uri,
            route,
        )
    });
    let github_scope = credential.github_route.as_ref().and_then(|route| {
        let scoped = route.repository_scope.as_ref()?;
        github_scope::is_selected_route(state.allowed_host, state.model_proxy, &parts.uri, route)
            .then_some((route, scoped))
    });
    if !github_scope::source_is_current(
        &parts.headers,
        github_scope.map(|(route, _)| route),
        credential.generation,
    ) {
        return (StatusCode::FORBIDDEN, "GitHub source connection changed").into_response();
    }
    let scope_read = state.allowed_host == Some("api.github.com")
        && parts.uri.path() == github_scope::ACCESS_PATH
        && parts.method == axum::http::Method::GET;
    let scoped_uri = if scope_read {
        let Some((_, scope)) = github_scope else {
            return StatusCode::NO_CONTENT.into_response();
        };
        Some(github_scope::repository_metadata_uri(scope))
    } else {
        None
    };
    let target = match (state.allowed_host, &credential.github_route) {
        (Some("api.github.com"), Some(route)) => state
            .target
            .join_github(scoped_uri.as_ref().unwrap_or(&parts.uri), route),
        (None, Some(route)) if !state.model_proxy => {
            state.target.join_selected_platform(&parts.uri, route)
        }
        _ => state.target.join(&parts.uri),
    };
    let target = match target {
        Ok(target) => target,
        Err(error) if github_client.is_some() => {
            return github_write::failure(
                StatusCode::BAD_REQUEST,
                &error.code,
                &error.detail,
                false,
                route,
                credential.generation,
                None,
            )
        }
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
    let normalized = state
        .model_proxy
        .then(|| {
            crate::upstream_compaction::normalize(&parts.method, &parts.uri, &parts.headers, &body)
        })
        .flatten();
    let body = normalized.as_deref().unwrap_or(&body);
    if body.len() > MAX_REQUEST_BYTES {
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            "proxy request body is too large",
        )
            .into_response();
    }
    let mut outbound = reqwest::Request::new(parts.method, target);
    if scope_read {
        github_scope::metadata_request_headers(outbound.headers_mut());
    } else {
        copy_request_headers(&parts.headers, outbound.headers_mut());
    }
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
    if github_client.is_some() {
        github_write::set_deadline(&mut outbound);
    }
    let client = github_client.unwrap_or(&state.client);
    let response = match client.execute(outbound).await {
        Ok(response) => response,
        Err(error) if github_client.is_some() => {
            return github_write::transport_error(&error, route, credential.generation)
        }
        Err(error) => {
            tracing::warn!(error = %error.without_url(), "credential proxy upstream request failed");
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
    if let Some((route, scope)) = github_scope.filter(|_| scope_read) {
        return github_scope::access_report(
            response,
            route,
            scope,
            credential.generation.unwrap_or_default(),
            credential.value.as_bytes(),
        )
        .await;
    }
    if github_client.is_some() {
        return github_write::complete_response(
            response,
            credential.value.as_bytes(),
            route,
            credential.generation,
        )
        .await;
    }
    // NyxID resolves GitHub write refusals using the original provider response.
    // The projected OAuth headers alone cannot distinguish scope from other refusals.
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

#[cfg(test)]
#[path = "upstream_nyxid_contract_tests.rs"]
mod nyxid_contract_tests;
#[cfg(test)]
#[path = "upstream_tests.rs"]
mod tests;
