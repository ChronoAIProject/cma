//! Client for the sandbox provider that isolates agent runtimes.
//!
//! Shared infrastructure rather than a kernel: every kernel runs inside a
//! sandbox, so this sits beside the kernel modules instead of inside one. It is
//! not part of the abstraction in `lib.rs` — that file stays free of anything a
//! particular deployment depends on.
//!
//! The wire contract follows the provider's published lifecycle API: an API key
//! in a header, camelCase bodies, 202 on create and 204 on delete.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub use crate::sandbox_reclaimer::BatchSandboxReclaimer;
use crate::{RuntimeHandle, RuntimeIdentity, RuntimeReservationIdentity};

/// Header carrying the provider API key.
const API_KEY_HEADER: &str = "OPEN-SANDBOX-API-KEY";

/// How long to wait on the provider.
///
/// Creating a sandbox is not a quick call: the provider schedules a pod and
/// pulls an image, which takes seconds on a warm node and minutes on a cold or
/// preempted one. A minute was enough for the warm case and nothing else, so
/// sessions failed intermittently — and the failure looked like the provider
/// being unreachable rather than the wait being too short.
///
/// Sits inside the caller's own provisioning deadline, so a provider that
/// never answers is still given up on rather than waited for indefinitely.
const PROVIDER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(240);

/// How many sandboxes to ask the provider for per page.
///
/// The provider's own default is 20, which an ordinary deployment exceeds, and
/// counts every sandbox the key can see. 100 was verified accepted and echoed
/// back in `pagination.pageSize`.
/// Requested rather than assumed: the loop reads the answer's own page size,
/// because the ceiling is the provider's and not ours.
const LIST_PAGE_SIZE: u32 = 100;

/// How many pages to follow before refusing to guess at the rest.
///
/// A provider answering `hasNextPage: true` for ever would otherwise make a
/// reconciliation sweep unbounded. 5,000 sandboxes is two orders of magnitude
/// above anything run here; reaching it means the provider is wrong.
const LIST_PAGE_LIMIT: u32 = 50;

/// How a sandbox is described at creation.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SandboxRequest {
    /// Image the sandbox runs.
    pub image: ImageSpec,
    /// Process started inside it.
    pub entrypoint: Vec<String>,
    /// Environment for that process.
    pub env: BTreeMap<String, String>,
    /// Hard resource caps.
    pub resource_limits: BTreeMap<String, String>,
    /// Seconds before the provider reclaims the sandbox.
    ///
    /// Deliberately has no `skip_serializing_if`: `None` must travel as a
    /// literal `null`, which the provider reads as "never expire". Omitting the
    /// field instead means "server default", which would quietly restore a
    /// timer and kill live sessions.
    pub timeout: Option<u64>,
    /// Free-form tags, used for correlation and cleanup.
    pub metadata: BTreeMap<String, String>,
}

/// Image reference for a sandbox.
#[derive(Debug, Clone, Serialize)]
pub struct ImageSpec {
    /// Image URI in standard form.
    pub uri: String,
}

/// Lifecycle facts, nested under `status` rather than reported at top level.
#[derive(Debug, Clone, Deserialize)]
pub struct SandboxStatus {
    /// Provider lifecycle state.
    #[serde(default)]
    pub state: String,
    /// Machine-readable reason for that state.
    #[serde(default)]
    pub reason: Option<String>,
}

/// A sandbox as the provider reports it.
///
/// Deliberately forward-compatible: fields the provider adds later are ignored
/// rather than breaking deserialization.
#[derive(Debug, Clone, Deserialize)]
pub struct Sandbox {
    /// Provider-assigned identifier.
    pub id: String,
    /// Current lifecycle status.
    #[serde(default)]
    pub status: Option<SandboxStatus>,
    /// Labels the creator attached, which is how a sandbox is traced back to
    /// the session that owns it when nothing else references it.
    #[serde(default)]
    pub metadata: std::collections::BTreeMap<String, String>,
}

/// How the provider pages a listing.
///
/// Every field defaults under this module's forward-compatibility rule: a
/// missing field cannot make a listing unreadable. `has_next_page` defaults to
/// `false`, so the loop stops rather than spinning.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pagination {
    #[serde(default)]
    pub page: u32,
    #[serde(default)]
    pub page_size: u32,
    #[serde(default)]
    pub total_items: u64,
    #[serde(default)]
    pub total_pages: u32,
    #[serde(default)]
    pub has_next_page: bool,
}

#[derive(Debug, Deserialize)]
struct Listing {
    #[serde(default)]
    items: Vec<Sandbox>,
    /// `None` is one page, not page zero of zero. The bare `{"items":[...]}`
    /// form is what the current code accepts and must keep working.
    #[serde(default)]
    pagination: Option<Pagination>,
}

impl Sandbox {
    /// Whether the sandbox is serving.
    pub fn is_running(&self) -> bool {
        self.status
            .as_ref()
            .map(|s| s.state.eq_ignore_ascii_case("running"))
            .unwrap_or(false)
    }
}

/// Where a port inside a sandbox can be reached.
#[derive(Debug, Clone, Deserialize)]
pub struct Endpoint {
    /// Host and path to reach the service.
    pub endpoint: String,
}

/// Failures talking to the provider.
#[derive(Debug, thiserror::Error)]
pub enum SandboxError {
    /// The provider could not be reached.
    #[error("sandbox provider unreachable: {0}")]
    Unreachable(String),
    /// The provider rejected the request.
    #[error("sandbox provider returned {status}: {detail}")]
    Rejected {
        /// HTTP status returned.
        status: u16,
        /// Body, truncated. Never contains the API key.
        detail: String,
    },
    /// The response did not match the expected shape.
    #[error("sandbox provider response not usable: {0}")]
    Malformed(String),
    /// The listing did not end within the pages this client will follow.
    ///
    /// Refused rather than returned short, because a caller cannot tell a
    /// truncated list from a complete one and every consumer of this treats a
    /// complete list as authoritative about what is running.
    #[error("sandbox listing did not end after {pages} pages of {page_size}")]
    Incomplete { pages: u32, page_size: u32 },
    /// The provider object exists, but is not the exact runtime authorised for
    /// a destructive operation.
    #[error("sandbox provider identity changed: {0}")]
    IdentityMismatch(String),
    /// The configured provider surface cannot perform the requested operation.
    #[error("sandbox provider does not support this operation: {0}")]
    Unsupported(String),
}

/// Thin client over the sandbox lifecycle API.
pub struct SandboxClient {
    base_url: String,
    api_key: String,
    http: reqwest::Client,
    reclaimer: Option<BatchSandboxReclaimer>,
}

/// Strip any scheme and trailing slash from a provider endpoint.
///
/// The provider returns a host and path on its own, which is not a URL until
/// something puts a scheme on it. Which scheme that is belongs to the caller,
/// because only the caller knows how the provider was reached.
fn bare_endpoint(endpoint: &str) -> &str {
    endpoint
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/')
}

/// What to do after decoding one page.
#[derive(Debug, PartialEq, Eq)]
enum PageStep {
    Stop,
    Next(u32),
    Refuse,
}

fn next_step(pagination: Option<&Pagination>, pages_read: u32) -> PageStep {
    match pagination {
        Some(page) if page.has_next_page && pages_read < LIST_PAGE_LIMIT => {
            PageStep::Next(pages_read + 1)
        }
        Some(page) if page.has_next_page => PageStep::Refuse,
        _ => PageStep::Stop,
    }
}

impl SandboxClient {
    /// Build a client.
    ///
    /// The key is held here and sent only as a header; it never appears in a
    /// URL, a log line, or an error body.
    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            api_key: api_key.into(),
            http: reqwest::Client::builder()
                .timeout(PROVIDER_TIMEOUT)
                .build()
                .expect("http client builds"),
            reclaimer: None,
        }
    }

    /// Add namespaced Kubernetes authority for exact discovery and deletion.
    #[must_use]
    pub fn with_reclaimer(mut self, reclaimer: BatchSandboxReclaimer) -> Self {
        self.reclaimer = Some(reclaimer);
        self
    }

    /// Delete only the exact provider identity, with Kubernetes preconditions.
    pub async fn delete_if_identity(&self, identity: &RuntimeIdentity) -> Result<(), SandboxError> {
        let reclaimer = self.reclaimer.as_ref().ok_or_else(|| {
            SandboxError::Unsupported(
                "no namespaced Kubernetes BatchSandbox reclaimer is configured".into(),
            )
        })?;
        reclaimer.delete_if_identity(identity).await
    }

    /// Discover every provider create for an exact durable reservation.
    pub async fn discover_runtime_candidates(
        &self,
        reservation: &RuntimeReservationIdentity,
    ) -> Result<Vec<RuntimeHandle>, SandboxError> {
        let reclaimer = self.reclaimer.as_ref().ok_or_else(|| {
            SandboxError::Unsupported(
                "no namespaced Kubernetes BatchSandbox discovery client is configured".into(),
            )
        })?;
        reclaimer.discover_runtime_candidates(reservation).await
    }

    /// Base URL this client talks to.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Turn a provider endpoint and a path into a URL on a sandbox's listener.
    ///
    /// The scheme is taken from the base URL the provider was reached on, the
    /// same way the terminal URL takes it. Hardcoding one instead makes the
    /// deployment's own configuration a lie: a provider addressed in-cluster
    /// over plain HTTP is not reachable over TLS, and every call to a sandbox
    /// — capturing a conversation to rotate it, most of all — fails with the
    /// provider reported unreachable when it was reachable all along.
    fn sandbox_url(&self, endpoint: &str, path: &str) -> String {
        let scheme = if self.base_url.starts_with("https://") {
            "https"
        } else {
            "http"
        };
        format!("{scheme}://{}{path}", bare_endpoint(endpoint))
    }

    /// Read from a path on a sandbox's own listener.
    ///
    /// The supervisor inside a sandbox is reached through the provider's
    /// proxy, which is authenticated, so these calls carry the provider key
    /// exactly as the provider's own endpoints do.
    ///
    /// `None` for a 204, which several supervisor endpoints use to say "there
    /// is nothing here" — distinct from an error, and a caller that conflated
    /// the two would act on absence as though it were failure.
    pub async fn get_from_sandbox(
        &self,
        endpoint: &str,
        path: &str,
    ) -> Result<Option<Vec<u8>>, SandboxError> {
        let url = self.sandbox_url(endpoint, path);
        let resp = self
            .http
            .get(&url)
            .header(API_KEY_HEADER, &self.api_key)
            .send()
            .await
            .map_err(|e| SandboxError::Unreachable(e.to_string()))?;
        if resp.status() == reqwest::StatusCode::NO_CONTENT {
            return Ok(None);
        }
        if !resp.status().is_success() {
            return Err(SandboxError::Rejected {
                status: resp.status().as_u16(),
                detail: format!("{path} on the sandbox refused the request"),
            });
        }
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| SandboxError::Unreachable(e.to_string()))?;
        Ok(Some(bytes.to_vec()))
    }

    /// Post bytes to a path on a sandbox's own listener.
    pub async fn post_to_sandbox(
        &self,
        endpoint: &str,
        path: &str,
        body: Vec<u8>,
    ) -> Result<(), SandboxError> {
        let url = self.sandbox_url(endpoint, path);
        let resp = self
            .http
            .post(&url)
            .header(API_KEY_HEADER, &self.api_key)
            .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
            .body(body)
            .send()
            .await
            .map_err(|e| SandboxError::Unreachable(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(SandboxError::Rejected {
                status: resp.status().as_u16(),
                detail: format!("{path} on the sandbox refused the request"),
            });
        }
        Ok(())
    }

    /// Every sandbox this key can see.
    ///
    /// Reconciliation needs the provider's view, because the whole point is to
    /// find runtimes our own records have lost track of. This follows every
    /// advertised page and refuses the listing rather than returning it short.
    ///
    /// The provider gives no stable ordering guarantee. Concurrent fleet
    /// changes can therefore move an item across a page boundary while this is
    /// reading, which can make that item absent from the result.
    pub async fn list(&self) -> Result<Vec<Sandbox>, SandboxError> {
        let mut sandboxes = Vec::new();
        let mut page = 1;
        let mut pages_read = 0;

        loop {
            let url = format!(
                "{}/v1/sandboxes?page={page}&pageSize={LIST_PAGE_SIZE}",
                self.base_url
            );
            let response = self
                .http
                .get(&url)
                .header(API_KEY_HEADER, &self.api_key)
                .send()
                .await
                .map_err(|e| SandboxError::Unreachable(format!("GET {url}: {e}")))?;
            let status = response.status();
            if !status.is_success() {
                let body = response.text().await.unwrap_or_default();
                return Err(SandboxError::Rejected {
                    status: status.as_u16(),
                    detail: format!("listing sandboxes page {page}: {}", truncate(&body)),
                });
            }

            let listing: Listing = response.json().await.map_err(|e| {
                SandboxError::Malformed(format!("sandbox listing page {page} not readable: {e}"))
            })?;
            pages_read += 1;
            let item_count = listing.items.len();
            if pages_read > 1 {
                tracing::debug!(page, returned = item_count, "sandbox listing page received");
            }

            let step = next_step(listing.pagination.as_ref(), pages_read);
            let page_size = listing
                .pagination
                .as_ref()
                .map_or(LIST_PAGE_SIZE, |pagination| pagination.page_size);
            let total_items = listing
                .pagination
                .as_ref()
                .map_or(sandboxes.len() as u64 + item_count as u64, |pagination| {
                    pagination.total_items
                });
            sandboxes.extend(listing.items);

            match step {
                PageStep::Stop => {
                    tracing::debug!(
                        total = sandboxes.len(),
                        pages = pages_read,
                        "sandbox listing completed"
                    );
                    return Ok(sandboxes);
                }
                PageStep::Next(next) => page = next,
                PageStep::Refuse => {
                    tracing::error!(
                        page_limit = LIST_PAGE_LIMIT,
                        total_items,
                        "sandbox listing exceeded pagination bound"
                    );
                    return Err(SandboxError::Incomplete {
                        pages: pages_read,
                        page_size,
                    });
                }
            }
        }
    }

    /// Create a sandbox.
    pub async fn create(&self, request: &SandboxRequest) -> Result<Sandbox, SandboxError> {
        let url = format!("{}/v1/sandboxes", self.base_url);
        let resp = self
            .http
            .post(&url)
            .header(API_KEY_HEADER, &self.api_key)
            .json(request)
            .send()
            .await
            .map_err(|e| SandboxError::Unreachable(format!("POST {url}: {e}")))?;
        Self::decode(resp).await
    }

    /// Read a sandbox back.
    pub async fn get(&self, id: &str) -> Result<Sandbox, SandboxError> {
        let url = format!("{}/v1/sandboxes/{id}", self.base_url);
        let resp = self
            .http
            .get(&url)
            .header(API_KEY_HEADER, &self.api_key)
            .send()
            .await
            .map_err(|e| SandboxError::Unreachable(format!("GET {url}: {e}")))?;
        Self::decode(resp).await
    }

    /// Resolve how to reach a port inside the sandbox.
    ///
    /// The server-proxied form is requested deliberately. The direct form
    /// returns a pod address that changes whenever the sandbox is rescheduled,
    /// and reaching it bypasses the provider's own access control.
    pub async fn endpoint(&self, id: &str, port: u16) -> Result<Endpoint, SandboxError> {
        let url = format!(
            "{}/v1/sandboxes/{id}/endpoints/{port}?use_server_proxy=true",
            self.base_url
        );
        let resp = self
            .http
            .get(&url)
            .header(API_KEY_HEADER, &self.api_key)
            .send()
            .await
            .map_err(|e| SandboxError::Unreachable(format!("GET {url}: {e}")))?;
        Self::decode(resp).await
    }

    /// Delete a sandbox. A sandbox already gone is not an error.
    pub async fn delete(&self, id: &str) -> Result<(), SandboxError> {
        let url = format!("{}/v1/sandboxes/{id}", self.base_url);
        let resp = self
            .http
            .delete(&url)
            .header(API_KEY_HEADER, &self.api_key)
            .send()
            .await
            .map_err(|e| SandboxError::Unreachable(format!("DELETE {url}: {e}")))?;
        let status = resp.status();
        if status.is_success() || status.as_u16() == 404 {
            return Ok(());
        }
        Err(SandboxError::Rejected {
            status: status.as_u16(),
            detail: truncate(&resp.text().await.unwrap_or_default()),
        })
    }

    /// The header name callers must set to reach a proxied endpoint.
    pub fn auth_header(&self) -> (&'static str, &str) {
        (API_KEY_HEADER, &self.api_key)
    }

    async fn decode<T: serde::de::DeserializeOwned>(
        resp: reqwest::Response,
    ) -> Result<T, SandboxError> {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(SandboxError::Rejected {
                status: status.as_u16(),
                detail: truncate(&body),
            });
        }
        serde_json::from_str(&body).map_err(|e| SandboxError::Malformed(format!("{e}")))
    }
}

/// Keep an error body short enough to log without dumping a response.
fn truncate(body: &str) -> String {
    const MAX: usize = 300;
    if body.len() <= MAX {
        return body.to_string();
    }
    format!("{}…", &body[..MAX])
}

#[cfg(test)]
#[path = "sandbox_tests.rs"]
mod sandbox_tests;
