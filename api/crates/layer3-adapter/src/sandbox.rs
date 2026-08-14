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
}

/// Thin client over the sandbox lifecycle API.
pub struct SandboxClient {
    base_url: String,
    api_key: String,
    http: reqwest::Client,
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
        }
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

    /// Create a sandbox.
    /// Every sandbox this key can see.
    ///
    /// Reconciliation needs the provider's view, because the whole point is to
    /// find runtimes our own records have lost track of.
    pub async fn list(&self) -> Result<Vec<Sandbox>, SandboxError> {
        let url = format!("{}/v1/sandboxes", self.base_url);
        let response = self
            .http
            .get(&url)
            .header(API_KEY_HEADER, &self.api_key)
            .send()
            .await
            .map_err(|e| SandboxError::Unreachable(format!("GET {url}: {e}")))?;
        if !response.status().is_success() {
            return Err(SandboxError::Rejected {
                status: response.status().as_u16(),
                detail: "listing sandboxes".to_string(),
            });
        }
        #[derive(serde::Deserialize)]
        struct Listing {
            #[serde(default)]
            items: Vec<Sandbox>,
        }
        let listing: Listing = response
            .json()
            .await
            .map_err(|e| SandboxError::Malformed(format!("sandbox listing not readable: {e}")))?;
        Ok(listing.items)
    }

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
mod tests {
    use super::*;

    #[test]
    fn base_url_loses_its_trailing_slash() {
        let c = SandboxClient::new("http://provider/", "k");
        assert_eq!(c.base_url(), "http://provider");
    }

    #[test]
    fn a_sandbox_is_called_on_the_scheme_the_provider_was_reached_on() {
        // The provider runs in-cluster on plain HTTP. Forcing TLS here made
        // every sandbox call fail as "provider unreachable", which stopped
        // rotation from ever capturing a conversation.
        let c = SandboxClient::new("http://provider.svc.cluster.local", "k");
        assert_eq!(
            c.sandbox_url(
                "provider.svc.cluster.local/proxy/9090",
                "/runtime/conversation"
            ),
            "http://provider.svc.cluster.local/proxy/9090/runtime/conversation"
        );

        let c = SandboxClient::new("https://provider.example.com", "k");
        assert_eq!(
            c.sandbox_url("provider.example.com/proxy/9090", "/runtime/conversation"),
            "https://provider.example.com/proxy/9090/runtime/conversation"
        );
    }

    #[test]
    fn an_endpoint_keeps_its_own_scheme_out_of_the_url() {
        // The provider may hand back the endpoint with a scheme already on it,
        // and it is not authoritative about how we reach it.
        let c = SandboxClient::new("http://provider", "k");
        for endpoint in ["https://host/proxy", "http://host/proxy", "host/proxy/"] {
            assert_eq!(c.sandbox_url(endpoint, "/p"), "http://host/proxy/p");
        }
    }

    #[test]
    fn the_api_key_travels_as_a_header_not_a_url() {
        let c = SandboxClient::new("http://provider", "super-secret");
        let (name, value) = c.auth_header();
        assert_eq!(name, "OPEN-SANDBOX-API-KEY");
        assert_eq!(value, "super-secret");
        assert!(
            !c.base_url().contains("super-secret"),
            "the key must never be part of a URL"
        );
    }

    #[test]
    fn a_running_sandbox_is_recognised_case_insensitively() {
        let s: Sandbox =
            serde_json::from_str(r#"{"id":"a","status":{"state":"Running","reason":"RUNNING"}}"#)
                .expect("parses");
        assert!(s.is_running());
    }

    #[test]
    fn a_pending_sandbox_is_not_running() {
        let s: Sandbox =
            serde_json::from_str(r#"{"id":"a","status":{"state":"Pending"}}"#).expect("parses");
        assert!(!s.is_running());

        let no_status: Sandbox = serde_json::from_str(r#"{"id":"a"}"#).expect("parses");
        assert!(!no_status.is_running());
    }

    #[test]
    fn unknown_response_fields_are_ignored() {
        // The provider adds fields over time; a new one must not break us.
        let s: Sandbox = serde_json::from_str(
            r#"{"id":"a","status":{"state":"Running","somethingNew":1},"expiresAt":"x"}"#,
        )
        .expect("forward-compatible");
        assert_eq!(s.id, "a");
    }

    #[test]
    fn a_request_serializes_with_camel_case_keys() {
        let r = SandboxRequest {
            image: ImageSpec {
                uri: "example/image:1".into(),
            },
            entrypoint: vec!["/bin/run".into()],
            env: BTreeMap::from([("A".to_string(), "1".to_string())]),
            resource_limits: BTreeMap::from([("cpu".to_string(), "1".to_string())]),
            timeout: Some(3600),
            metadata: BTreeMap::new(),
        };
        let json = serde_json::to_value(&r).expect("serializes");
        assert!(json.get("resourceLimits").is_some(), "got {json}");
        assert_eq!(json["image"]["uri"], "example/image:1");
    }

    #[test]
    fn an_absent_timeout_serializes_as_null_not_omitted() {
        let r = SandboxRequest {
            image: ImageSpec { uri: "x".into() },
            entrypoint: vec![],
            env: BTreeMap::new(),
            resource_limits: BTreeMap::new(),
            timeout: None,
            metadata: BTreeMap::new(),
        };
        let json = serde_json::to_value(&r).expect("serializes");
        assert!(
            json.get("timeout").is_some(),
            "timeout must be present as null, not omitted: {json}"
        );
        assert!(json["timeout"].is_null(), "got {json}");
    }

    #[test]
    fn long_error_bodies_are_truncated() {
        let long = "x".repeat(1000);
        let t = truncate(&long);
        assert!(t.len() < 400, "got {} chars", t.len());
        assert!(t.ends_with('…'));
        assert_eq!(truncate("short"), "short");
    }
}
