//! Shared async HTTP client and resource composition.

use std::{fmt, sync::Arc, time::Duration};

use reqwest::{Method, StatusCode, Url, header::HeaderValue};
use serde::{Serialize, de::DeserializeOwned};

use crate::http::{Request, send};
use crate::{Conformance, Error, IdempotencyKey, Result};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
const USER_AGENT: &str = concat!("cma-sdk/", env!("CARGO_PKG_VERSION"));

/// Builder for a [`Client`].
pub struct ClientBuilder {
    base_url: Url,
    request_timeout: Duration,
    bearer: Option<HeaderValue>,
}

impl ClientBuilder {
    /// Set the deadline applied to each request, including its complete body.
    pub fn request_timeout(mut self, timeout: Duration) -> Result<Self> {
        if timeout.is_zero() {
            return Err(Error::protocol("request timeout must be non-zero"));
        }
        self.request_timeout = timeout;
        Ok(self)
    }

    /// Configure a bearer credential for resource requests, never anonymous discovery.
    pub fn bearer_token(mut self, token: impl AsRef<str>) -> Result<Self> {
        let token = token.as_ref();
        if token.is_empty() || !token.bytes().all(|b| (33..=126).contains(&b)) {
            return Err(Error::validation(
                "bearer token must contain only visible ASCII without spaces",
            ));
        }
        let mut header = HeaderValue::from_str(&format!("Bearer {token}"))
            .map_err(|_| Error::validation("bearer token is not a valid header"))?;
        header.set_sensitive(true);
        self.bearer = Some(header);
        Ok(self)
    }

    /// Build the rustls-backed transport without redirect following or automatic retries.
    pub fn build(self) -> Result<Client> {
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .build()
            .map_err(|_| Error::transport("could not build HTTP transport"))?;
        Ok(Client {
            inner: Arc::new(ClientInner {
                http,
                base_url: self.base_url,
                request_timeout: self.request_timeout,
                bearer: self.bearer,
            }),
        })
    }
}

impl fmt::Debug for ClientBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClientBuilder")
            .field("base_url", &self.base_url)
            .field("request_timeout", &self.request_timeout)
            .field("authenticated", &self.bearer.is_some())
            .finish()
    }
}

/// Cloneable async client for CMA discovery and typed REST resources.
#[derive(Clone)]
pub struct Client {
    pub(crate) inner: Arc<ClientInner>,
}

pub(crate) struct ClientInner {
    pub(crate) http: reqwest::Client,
    pub(crate) base_url: Url,
    pub(crate) request_timeout: Duration,
    pub(crate) bearer: Option<HeaderValue>,
}

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Client")
            .field("base_url", &self.inner.base_url)
            .field("request_timeout", &self.inner.request_timeout)
            .field("authenticated", &self.inner.bearer.is_some())
            .finish()
    }
}

impl Client {
    /// Start configuring a client for an HTTP or HTTPS deployment root.
    pub fn builder(base_url: impl AsRef<str>) -> Result<ClientBuilder> {
        let mut base_url =
            Url::parse(base_url.as_ref()).map_err(|_| Error::protocol("invalid CMA base URL"))?;
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
            bearer: None,
        })
    }

    /// Read the deployment's anonymous public state without sending a bearer credential.
    pub async fn read_conformance(&self) -> Result<Conformance> {
        let mut request = Request::get(&["conformance"]);
        request.anonymous = true;
        self.request::<Conformance>(request).await?.validate()
    }

    pub(crate) async fn request<T: DeserializeOwned>(&self, request: Request<'_>) -> Result<T> {
        let body = send(&self.inner, request).await?;
        serde_json::from_slice(&body)
            .map_err(|error| Error::protocol(format!("invalid response document: {error}")))
    }

    pub(crate) async fn get<T: DeserializeOwned>(
        &self,
        segments: &[&str],
        query: &[(&str, String)],
    ) -> Result<T> {
        let mut request = Request::get(segments);
        request.query = query;
        self.request(request).await
    }

    pub(crate) async fn command<T: DeserializeOwned, B: Serialize>(
        &self,
        method: Method,
        segments: &[&str],
        body: Option<&B>,
        key: &IdempotencyKey,
        status: StatusCode,
    ) -> Result<T> {
        self.request(Request::command(method, segments, body, key, status)?)
            .await
    }

    pub(crate) async fn command_void<B: Serialize>(
        &self,
        method: Method,
        segments: &[&str],
        body: Option<&B>,
        key: &IdempotencyKey,
    ) -> Result<()> {
        let bytes = send(
            &self.inner,
            Request::command(method, segments, body, key, StatusCode::ACCEPTED)?,
        )
        .await?;
        if bytes.iter().all(u8::is_ascii_whitespace) {
            Ok(())
        } else {
            Err(Error::protocol(
                "bodyless operation unexpectedly returned a body",
            ))
        }
    }

    pub(crate) async fn operation<T: DeserializeOwned, B: Serialize>(
        &self,
        method: Method,
        segments: &[&str],
        body: Option<&B>,
        status: StatusCode,
    ) -> Result<T> {
        self.request(Request::operation(method, segments, body, status)?)
            .await
    }

    pub(crate) async fn empty(&self, request: Request<'_>) -> Result<()> {
        let body = send(&self.inner, request).await?;
        if body.iter().all(u8::is_ascii_whitespace) {
            Ok(())
        } else {
            Err(Error::protocol(
                "bodyless operation unexpectedly returned a body",
            ))
        }
    }

    pub(crate) async fn refusal(&self, request: Request<'_>) -> Result<()> {
        send(&self.inner, request).await?;
        Err(Error::protocol(
            "unsupported operation unexpectedly returned success",
        ))
    }
}
