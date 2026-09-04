use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;

use super::reclaimer::CapturedSandbox;
use super::{
    BatchSandboxReclaimer, Pagination, ProviderEndpoint, ProviderError, ProviderSandbox,
    ProviderSandboxRequest,
};
use crate::RuntimeHandle;

const API_KEY_HEADER: &str = "OPEN-SANDBOX-API-KEY";
const PROVIDER_TIMEOUT: Duration = Duration::from_secs(240);
const LIST_PAGE_SIZE: u32 = 100;
const LIST_PAGE_LIMIT: u32 = 50;

#[derive(Debug, Deserialize)]
struct Listing {
    #[serde(default)]
    items: Vec<ProviderSandbox>,
    #[serde(default)]
    pagination: Option<Pagination>,
}

#[derive(Clone)]
pub(crate) enum ProviderDeleteTarget {
    Fenced {
        reclaimer: Box<BatchSandboxReclaimer>,
        captured: CapturedSandbox,
    },
    Unfenced(RuntimeHandle),
}

/// Thin client over the OpenSandbox lifecycle API.
#[derive(Clone)]
pub struct SandboxClient {
    base_url: String,
    api_key: Arc<str>,
    http: reqwest::Client,
    reclaimer: Option<BatchSandboxReclaimer>,
}

impl SandboxClient {
    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            api_key: Arc::from(api_key.into()),
            http: reqwest::Client::builder()
                .timeout(PROVIDER_TIMEOUT)
                .build()
                .expect("provider HTTP client builds"),
            reclaimer: None,
        }
    }

    #[must_use]
    pub fn with_reclaimer(mut self, reclaimer: BatchSandboxReclaimer) -> Self {
        self.reclaimer = Some(reclaimer);
        self
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn auth_header(&self) -> (&'static str, &str) {
        (API_KEY_HEADER, &self.api_key)
    }

    pub async fn create(
        &self,
        request: &ProviderSandboxRequest,
    ) -> Result<ProviderSandbox, ProviderError> {
        let url = format!("{}/v1/sandboxes", self.base_url);
        let response = self
            .http
            .post(&url)
            .header(API_KEY_HEADER, self.api_key.as_ref())
            .json(request)
            .send()
            .await
            .map_err(|error| ProviderError::Unreachable(format!("POST {url}: {error}")))?;
        decode(response).await
    }

    pub async fn get(&self, id: &str) -> Result<ProviderSandbox, ProviderError> {
        let url = format!("{}/v1/sandboxes/{id}", self.base_url);
        let response = self
            .http
            .get(&url)
            .header(API_KEY_HEADER, self.api_key.as_ref())
            .send()
            .await
            .map_err(|error| ProviderError::Unreachable(format!("GET {url}: {error}")))?;
        decode(response).await
    }

    pub async fn endpoint(&self, id: &str, port: u16) -> Result<ProviderEndpoint, ProviderError> {
        let url = format!(
            "{}/v1/sandboxes/{id}/endpoints/{port}?use_server_proxy=true",
            self.base_url
        );
        let response = self
            .http
            .get(&url)
            .header(API_KEY_HEADER, self.api_key.as_ref())
            .send()
            .await
            .map_err(|error| ProviderError::Unreachable(format!("GET {url}: {error}")))?;
        decode(response).await
    }

    /// Follow every advertised page or refuse the whole inventory.
    pub async fn list(&self) -> Result<Vec<ProviderSandbox>, ProviderError> {
        let mut answer = Vec::new();
        for pages_read in 1..=LIST_PAGE_LIMIT {
            let url = format!(
                "{}/v1/sandboxes?page={pages_read}&pageSize={LIST_PAGE_SIZE}",
                self.base_url
            );
            let response = self
                .http
                .get(&url)
                .header(API_KEY_HEADER, self.api_key.as_ref())
                .send()
                .await
                .map_err(|error| ProviderError::Unreachable(format!("GET {url}: {error}")))?;
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            if !status.is_success() {
                return Err(ProviderError::Rejected {
                    status: status.as_u16(),
                    detail: format!("listing sandboxes page {pages_read}: {}", truncate(&body)),
                });
            }
            let listing: Listing = serde_json::from_str(&body).map_err(|error| {
                ProviderError::Malformed(format!(
                    "sandbox listing page {pages_read} not readable: {error}"
                ))
            })?;
            let has_next = listing
                .pagination
                .as_ref()
                .is_some_and(|page| page.has_next_page);
            let page_size = listing
                .pagination
                .as_ref()
                .map_or(LIST_PAGE_SIZE, |page| page.page_size);
            answer.extend(listing.items);
            if !has_next {
                return Ok(answer);
            }
            if pages_read == LIST_PAGE_LIMIT {
                return Err(ProviderError::Incomplete {
                    pages: pages_read,
                    page_size,
                });
            }
        }
        unreachable!("bounded provider pagination always returns")
    }

    pub(crate) async fn capture_delete_target(
        &self,
        handle: &RuntimeHandle,
    ) -> Result<Option<ProviderDeleteTarget>, ProviderError> {
        let Some(reclaimer) = &self.reclaimer else {
            return Ok(Some(ProviderDeleteTarget::Unfenced(handle.clone())));
        };
        Ok(reclaimer
            .capture(handle)
            .await?
            .map(|captured| ProviderDeleteTarget::Fenced {
                reclaimer: Box::new(reclaimer.clone()),
                captured,
            }))
    }

    pub(crate) async fn delete_captured(
        &self,
        target: ProviderDeleteTarget,
    ) -> Result<(), ProviderError> {
        match target {
            ProviderDeleteTarget::Fenced {
                reclaimer,
                captured,
            } => reclaimer.delete(captured).await,
            ProviderDeleteTarget::Unfenced(handle) => self.delete_unfenced(&handle).await,
        }
    }

    pub async fn delete(&self, handle: &RuntimeHandle) -> Result<(), ProviderError> {
        let Some(target) = self.capture_delete_target(handle).await? else {
            return Ok(());
        };
        self.delete_captured(target).await
    }

    async fn delete_unfenced(&self, handle: &RuntimeHandle) -> Result<(), ProviderError> {
        let url = format!("{}/v1/sandboxes/{}", self.base_url, handle.0);
        let response = self
            .http
            .delete(&url)
            .header(API_KEY_HEADER, self.api_key.as_ref())
            .send()
            .await
            .map_err(|error| ProviderError::Unreachable(format!("DELETE {url}: {error}")))?;
        let status = response.status();
        if status.is_success() || status == reqwest::StatusCode::NOT_FOUND {
            return Ok(());
        }
        Err(ProviderError::Rejected {
            status: status.as_u16(),
            detail: truncate(&response.text().await.unwrap_or_default()),
        })
    }
}

async fn decode<T: serde::de::DeserializeOwned>(
    response: reqwest::Response,
) -> Result<T, ProviderError> {
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(ProviderError::Rejected {
            status: status.as_u16(),
            detail: truncate(&body),
        });
    }
    serde_json::from_str(&body).map_err(|error| ProviderError::Malformed(error.to_string()))
}

fn truncate(body: &str) -> String {
    const MAX_CHARS: usize = 300;
    let mut chars = body.chars();
    let prefix: String = chars.by_ref().take(MAX_CHARS).collect();
    if chars.next().is_some() {
        format!("{prefix}...")
    } else {
        prefix
    }
}
