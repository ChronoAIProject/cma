//! HTTP client for CMA's temporary discovery boundary.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use reqwest::{StatusCode, Url};
use serde::Deserialize;

use crate::{Conformance, Error, Result};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_RESPONSE_BYTES: u64 = 8 * 1024 * 1024;
const USER_AGENT: &str = concat!("cma-sdk/", env!("CARGO_PKG_VERSION"));

/// Builder for a [`Client`].
pub struct ClientBuilder {
    base_url: Url,
    request_timeout: Duration,
}

impl ClientBuilder {
    /// Set the deadline applied to the discovery request.
    pub fn request_timeout(mut self, timeout: Duration) -> Result<Self> {
        if timeout.is_zero() {
            return Err(Error::protocol("request timeout must be non-zero"));
        }
        self.request_timeout = timeout;
        Ok(self)
    }

    /// Build the rustls-backed HTTP client.
    pub fn build(self) -> Result<Client> {
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| {
                Error::transport(format!("could not build HTTP transport: {error}"))
            })?;
        Ok(Client {
            inner: Arc::new(ClientInner {
                http,
                base_url: self.base_url,
                request_timeout: self.request_timeout,
            }),
        })
    }
}

impl fmt::Debug for ClientBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClientBuilder")
            .field("base_url", &self.base_url)
            .field("request_timeout", &self.request_timeout)
            .finish()
    }
}

/// Async client for CMA's anonymous deployment discovery route.
#[derive(Clone)]
pub struct Client {
    inner: Arc<ClientInner>,
}

struct ClientInner {
    http: reqwest::Client,
    base_url: Url,
    request_timeout: Duration,
}

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Client")
            .field("base_url", &self.inner.base_url)
            .field("request_timeout", &self.inner.request_timeout)
            .finish()
    }
}

impl Client {
    /// Start configuring a client for an HTTP or HTTPS deployment root.
    pub fn builder(base_url: impl AsRef<str>) -> Result<ClientBuilder> {
        let mut base_url = Url::parse(base_url.as_ref())
            .map_err(|error| Error::protocol(format!("invalid CMA base URL: {error}")))?;
        if !matches!(base_url.scheme(), "http" | "https") {
            return Err(Error::protocol("CMA base URL must use HTTP or HTTPS"));
        }
        if base_url.host_str().is_none() {
            return Err(Error::protocol("CMA base URL requires a host"));
        }
        if !base_url.username().is_empty() || base_url.password().is_some() {
            return Err(Error::protocol(
                "CMA base URL must not contain user information",
            ));
        }
        if base_url.query().is_some() || base_url.fragment().is_some() {
            return Err(Error::protocol(
                "CMA base URL must not contain a query or fragment",
            ));
        }
        if !base_url.path().ends_with('/') {
            base_url.set_path(&format!("{}/", base_url.path()));
        }
        Ok(ClientBuilder {
            base_url,
            request_timeout: DEFAULT_TIMEOUT,
        })
    }

    /// Read the deployment's anonymous public state.
    pub async fn read_conformance(&self) -> Result<Conformance> {
        let endpoint = self
            .inner
            .base_url
            .join("api/v1/conformance")
            .map_err(|error| Error::protocol(format!("invalid discovery endpoint: {error}")))?;
        let response = self
            .inner
            .http
            .get(endpoint)
            .timeout(self.inner.request_timeout)
            .send()
            .await
            .map_err(|error| Error::transport(format!("discovery request failed: {error}")))?;
        let status = response.status();
        let request_id = response_request_id(&response);
        let body = response_body(response).await?;

        if status != StatusCode::OK {
            return Err(problem_error(status, request_id, &body));
        }
        let document: Conformance = serde_json::from_slice(&body)
            .map_err(|error| Error::protocol(format!("invalid conformance document: {error}")))?;
        document.validate()
    }
}

async fn response_body(response: reqwest::Response) -> Result<Vec<u8>> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE_BYTES)
    {
        return Err(Error::protocol(format!(
            "response exceeds the {MAX_RESPONSE_BYTES}-byte boundary"
        )));
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|error| Error::transport(format!("could not read response body: {error}")))?;
    if bytes.len() as u64 > MAX_RESPONSE_BYTES {
        return Err(Error::protocol(format!(
            "response exceeds the {MAX_RESPONSE_BYTES}-byte boundary"
        )));
    }
    Ok(bytes.to_vec())
}

#[derive(Deserialize)]
struct Problem {
    status: Option<u16>,
    code: Option<String>,
    detail: Option<String>,
    title: Option<String>,
}

fn problem_error(status: StatusCode, request_id: Option<String>, body: &[u8]) -> Error {
    let problem: Problem = match serde_json::from_slice(body) {
        Ok(problem) => problem,
        Err(error) => {
            return Error::protocol(format!(
                "HTTP {} did not carry a machine-readable problem document: {error}",
                status.as_u16()
            ));
        }
    };
    if problem
        .status
        .is_some_and(|declared| declared != status.as_u16())
    {
        return Error::protocol(format!(
            "problem status did not match HTTP {}",
            status.as_u16()
        ));
    }
    let message = problem
        .detail
        .filter(|value| !value.trim().is_empty())
        .or_else(|| problem.title.filter(|value| !value.trim().is_empty()));
    let Some(message) = message else {
        return Error::protocol(format!(
            "HTTP {} problem document carried no detail",
            status.as_u16()
        ));
    };
    Error::Api {
        status: status.as_u16(),
        code: problem.code.filter(|value| !value.trim().is_empty()),
        message,
        request_id,
    }
}

fn response_request_id(response: &reqwest::Response) -> Option<String> {
    response
        .headers()
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned)
}
