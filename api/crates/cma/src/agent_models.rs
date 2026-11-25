//! The public Chrono LLM catalogue, authenticated with exact Sandbox custody.
use std::{
    collections::BTreeSet,
    sync::Arc,
    time::{Duration, SystemTime},
};

use futures_util::StreamExt as _;
use layer1_public::rest::{AgentModelCatalog, AgentModelCatalogError, AgentModelView};
use layer2_core::dependency_diagnostics::{Dependency, Failure, Reason};
use layer2_core::{sandbox::Sandbox, seconds_from_rfc3339};
use serde::Deserialize;

use crate::sandbox::SandboxCredentialController;

mod cache;
pub(crate) use cache::CachedModelSupport;
#[cfg(test)]
#[path = "agent_models/cache/test_runtime.rs"]
pub(crate) mod test_runtime;

const MAX_BYTES: usize = 1024 * 1024;
const MAX_MODELS: usize = 1000;
const USER_AGENT: &str = concat!("chrono-cma/", env!("CARGO_PKG_VERSION"));

pub(crate) struct ChronoAgentModels {
    http: reqwest::Client,
    origin: reqwest::Url,
    credentials: Arc<dyn SandboxCredentialController>,
    cache: Option<cache::CatalogCache>,
}

impl ChronoAgentModels {
    pub(crate) fn new(
        origin: &str,
        credentials: Arc<dyn SandboxCredentialController>,
    ) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .redirect(reqwest::redirect::Policy::none())
                .user_agent(USER_AGENT)
                .build()
                .map_err(|_| "model catalogue HTTP client is unavailable")?,
            origin: reqwest::Url::parse(origin).map_err(|_| "model catalogue origin is invalid")?,
            credentials,
            cache: None,
        }))
    }

    pub(crate) fn with_cache(self: Arc<Self>, enabled: bool) -> Arc<Self> {
        if !enabled {
            return self;
        }
        Arc::new(Self {
            http: self.http.clone(),
            origin: self.origin.clone(),
            credentials: self.credentials.clone(),
            cache: Some(cache::CatalogCache::new()),
        })
    }

    async fn fetch(
        &self,
        service: &str,
        bearer: &str,
    ) -> Result<Vec<AgentModelView>, AgentModelCatalogError> {
        let mut url = self.origin.clone();
        url.path_segments_mut()
            .map_err(|_| unavailable(Dependency::Local, Reason::Configuration))?
            .extend(["api", "v1", "proxy", "s", service, "models"]);
        let response = self
            .http
            .get(url)
            .header(reqwest::header::ACCEPT, "application/json")
            .bearer_auth(bearer)
            .send()
            .await
            .map_err(|error| unavailable(Dependency::ModelGateway, transport_reason(&error)))?;
        if response.status() != reqwest::StatusCode::OK {
            let reason = match response.status().as_u16() {
                502 => Reason::BadGateway,
                503 => Reason::UpstreamUnavailable,
                429 => Reason::RateLimited,
                _ => Reason::UnexpectedStatus,
            };
            return Err(unavailable_with_status(
                Dependency::ModelGateway,
                reason,
                Some(response.status().as_u16()),
            ));
        }
        if response
            .content_length()
            .is_some_and(|n| n > MAX_BYTES as u64)
        {
            return Err(catalogue_body_unavailable(Reason::ResponseTooLarge));
        }
        let mut body = Vec::new();
        let mut chunks = response.bytes_stream();
        while let Some(chunk) = chunks.next().await {
            let chunk = chunk.map_err(|error| {
                catalogue_body_unavailable(if error.is_timeout() {
                    Reason::BodyTimeout
                } else {
                    Reason::ResponseBody
                })
            })?;
            if body.len().saturating_add(chunk.len()) > MAX_BYTES {
                return Err(catalogue_body_unavailable(Reason::ResponseTooLarge));
            }
            body.extend_from_slice(&chunk);
        }
        parse(&body)
    }
}

#[async_trait::async_trait]
impl AgentModelCatalog for ChronoAgentModels {
    async fn models(
        &self,
        sandbox: &Sandbox,
    ) -> Result<Vec<AgentModelView>, AgentModelCatalogError> {
        if sandbox.validate().is_err() {
            return Err(unavailable(Dependency::Storage, Reason::Corrupt));
        }
        if !sandbox.credential.delivered {
            return Err(unavailable(
                Dependency::AuthenticationAuthority,
                Reason::CredentialMissing,
            ));
        }
        let reaches = sandbox.credential.effective_reaches();
        let service = ["chrono-llm-public", "chrono-llm"]
            .into_iter()
            .find(|name| reaches.iter().any(|s| s.as_str() == *name))
            .ok_or_else(|| {
                unavailable(Dependency::AuthenticationAuthority, Reason::ScopeMissing)
            })?;
        let current = sandbox.credential.current.as_ref().ok_or_else(|| {
            unavailable(
                Dependency::AuthenticationAuthority,
                Reason::CredentialMissing,
            )
        })?;
        validate_expiry(sandbox)?;
        let material = self
            .credentials
            .material(&sandbox.id, current.generation)
            .await
            .map_err(|error| {
                unavailable(
                    Dependency::Storage,
                    match error {
                        crate::sandbox::CredentialError::Security(_) => Reason::Corrupt,
                        _ => Reason::Unavailable,
                    },
                )
            })?
            .ok_or_else(|| unavailable(Dependency::Storage, Reason::CredentialMissing))?;
        if material.owner != sandbox.owner
            || material.sandbox_id != sandbox.id
            || &material.authority != current
            || material.source != sandbox.credential.source
            || !material.matches_source()
            || material.value.expose().is_empty()
        {
            return Err(unavailable(
                Dependency::AuthenticationAuthority,
                Reason::CredentialMismatch,
            ));
        }
        let fetch = || async {
            validate_expiry(sandbox)?;
            self.fetch(service, material.value.expose()).await
        };
        let result = match &self.cache {
            Some(cache) => cache.get(sandbox, service, fetch).await,
            None => fetch().await,
        };
        // A custody read or a shared in-flight fetch may outlive the admitted
        // credential. A cache hit cannot extend its original validity.
        validate_expiry(sandbox)?;
        result
    }
}

fn validate_expiry(sandbox: &Sandbox) -> Result<(), AgentModelCatalogError> {
    if sandbox.credential.source.is_managed() {
        let expiry = sandbox
            .credential
            .current
            .as_ref()
            .and_then(|current| seconds_from_rfc3339(&current.expires_at))
            .ok_or_else(|| {
                unavailable(Dependency::AuthenticationAuthority, Reason::InvalidEvidence)
            })?;
        if SystemTime::now() >= SystemTime::UNIX_EPOCH + Duration::from_secs(expiry) {
            return Err(unavailable(
                Dependency::AuthenticationAuthority,
                Reason::CredentialExpired,
            ));
        }
    }
    Ok(())
}

#[derive(Deserialize)]
struct Listing {
    data: Vec<Model>,
}
#[derive(Deserialize)]
struct Model {
    id: String,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    name: Option<String>,
}

fn parse(body: &[u8]) -> Result<Vec<AgentModelView>, AgentModelCatalogError> {
    let listing: Listing = serde_json::from_slice(body)
        .map_err(|_| catalogue_body_unavailable(Reason::InvalidEvidence))?;
    if listing.data.len() > MAX_MODELS {
        return Err(catalogue_body_unavailable(Reason::InvalidEvidence));
    }
    let mut ids = BTreeSet::new();
    let mut models = Vec::with_capacity(listing.data.len());
    for model in listing.data {
        if !valid_text(&model.id) || !ids.insert(model.id.clone()) {
            return Err(catalogue_body_unavailable(Reason::InvalidEvidence));
        }
        if !agent_model(&model.id) {
            continue;
        }
        let label = model
            .display_name
            .filter(|name| valid_text(name))
            .or_else(|| model.name.filter(|name| valid_text(name)))
            .unwrap_or_else(|| model.id.clone());
        models.push(AgentModelView {
            id: model.id,
            label,
            available: false,
        });
    }
    models.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(models)
}

/// Chrono's catalogue mixes text and image models without capability metadata.
/// Keep this admitted ID contract aligned with the managed ACP catalogue:
/// ^gpt-[0-9][A-Za-z0-9._-]*$ or codex-auto-review. Entries are still discovered
/// from the exact Sandbox service; this predicate never adds catalogue rows.
fn agent_model(id: &str) -> bool {
    id == "codex-auto-review"
        || id.strip_prefix("gpt-").is_some_and(|suffix| {
            suffix.as_bytes().first().is_some_and(u8::is_ascii_digit)
                && suffix
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        })
}
fn valid_text(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value.trim() == value
        && !value.chars().any(char::is_control)
}
fn catalogue_body_unavailable(reason: Reason) -> AgentModelCatalogError {
    unavailable_with_status(Dependency::ModelGateway, reason, Some(200))
}

fn unavailable(dependency: Dependency, reason: Reason) -> AgentModelCatalogError {
    unavailable_with_status(dependency, reason, None)
}

fn unavailable_with_status(
    dependency: Dependency,
    reason: Reason,
    upstream_status: Option<u16>,
) -> AgentModelCatalogError {
    Failure::new(dependency, reason).record();
    // Keep the aggregate failure counter unchanged. This event supplies bounded
    // request-local detail before the public projection discards the cause.
    tracing::warn!(
        dependency = dependency.label(),
        reason = reason.label(),
        upstream_status,
        "model catalogue unavailable"
    );
    AgentModelCatalogError::Unavailable
}

fn transport_reason(error: &reqwest::Error) -> Reason {
    if error.is_timeout() {
        Reason::Timeout
    } else if error.is_connect() {
        Reason::Connection
    } else {
        Reason::Transport
    }
}

#[cfg(test)]
#[path = "agent_models/diagnostic_tests.rs"]
mod diagnostic_tests;
#[cfg(test)]
#[path = "agent_models/nyxid_contract_tests.rs"]
mod nyxid_contract_tests;
#[cfg(test)]
mod tests;
