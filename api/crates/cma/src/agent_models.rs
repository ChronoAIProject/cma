//! The public Chrono LLM catalogue, authenticated with exact Sandbox custody.
use std::{
    collections::BTreeSet,
    sync::Arc,
    time::{Duration, SystemTime},
};

use futures_util::StreamExt as _;
use layer1_public::rest::{AgentModelCatalog, AgentModelCatalogError, AgentModelView};
use layer2_core::{sandbox::Sandbox, seconds_from_rfc3339};
use serde::Deserialize;

use crate::sandbox::SandboxCredentialController;

const MAX_BYTES: usize = 1024 * 1024;
const MAX_MODELS: usize = 1000;

pub(crate) struct ChronoAgentModels {
    http: reqwest::Client,
    origin: reqwest::Url,
    credentials: Arc<dyn SandboxCredentialController>,
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
                .build()
                .map_err(|_| "model catalogue HTTP client is unavailable")?,
            origin: reqwest::Url::parse(origin).map_err(|_| "model catalogue origin is invalid")?,
            credentials,
        }))
    }

    async fn fetch(
        &self,
        service: &str,
        bearer: &str,
    ) -> Result<Vec<AgentModelView>, AgentModelCatalogError> {
        let mut url = self.origin.clone();
        url.path_segments_mut()
            .map_err(|_| unavailable())?
            .extend(["api", "v1", "proxy", "s", service, "models"]);
        let response = self
            .http
            .get(url)
            .bearer_auth(bearer)
            .send()
            .await
            .map_err(|_| unavailable())?;
        if response.status() != reqwest::StatusCode::OK
            || response
                .content_length()
                .is_some_and(|n| n > MAX_BYTES as u64)
        {
            return Err(unavailable());
        }
        let mut body = Vec::new();
        let mut chunks = response.bytes_stream();
        while let Some(chunk) = chunks.next().await {
            let chunk = chunk.map_err(|_| unavailable())?;
            if body.len().saturating_add(chunk.len()) > MAX_BYTES {
                return Err(unavailable());
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
        if sandbox.validate().is_err() || !sandbox.credential.delivered {
            return Err(unavailable());
        }
        let reaches = sandbox.credential.effective_reaches();
        let service = ["chrono-llm-public", "chrono-llm"]
            .into_iter()
            .find(|name| reaches.iter().any(|s| s.as_str() == *name))
            .ok_or_else(unavailable)?;
        let current = sandbox
            .credential
            .current
            .as_ref()
            .ok_or_else(unavailable)?;
        let expiry = seconds_from_rfc3339(&current.expires_at).ok_or_else(unavailable)?;
        if SystemTime::now() >= SystemTime::UNIX_EPOCH + Duration::from_secs(expiry) {
            return Err(unavailable());
        }
        let material = self
            .credentials
            .material(&sandbox.id, current.generation)
            .await
            .map_err(|_| unavailable())?
            .ok_or_else(unavailable)?;
        if material.owner != sandbox.owner
            || material.sandbox_id != sandbox.id
            || &material.authority != current
            || material.source != sandbox.credential.source
            || !material.matches_source()
            || material.value.expose().is_empty()
        {
            return Err(unavailable());
        }
        self.fetch(service, material.value.expose()).await
    }
}

#[derive(Deserialize)]
struct Listing {
    data: Vec<Model>,
}
#[derive(Deserialize)]
struct Model {
    id: String,
    #[serde(default)]
    name: Option<String>,
}

fn parse(body: &[u8]) -> Result<Vec<AgentModelView>, AgentModelCatalogError> {
    let listing: Listing = serde_json::from_slice(body).map_err(|_| unavailable())?;
    if listing.data.len() > MAX_MODELS {
        return Err(unavailable());
    }
    let mut ids = BTreeSet::new();
    let mut models = Vec::with_capacity(listing.data.len());
    for model in listing.data {
        if !valid_text(&model.id) || !ids.insert(model.id.clone()) {
            return Err(unavailable());
        }
        let label = model
            .name
            .filter(|name| valid_text(name))
            .unwrap_or_else(|| model.id.clone());
        models.push(AgentModelView {
            id: model.id,
            label,
        });
    }
    models.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(models)
}
fn valid_text(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value.trim() == value
        && !value.chars().any(char::is_control)
}
fn unavailable() -> AgentModelCatalogError {
    AgentModelCatalogError::Unavailable
}

#[cfg(test)]
mod tests;
