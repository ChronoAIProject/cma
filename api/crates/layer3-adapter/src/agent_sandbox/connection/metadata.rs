//! Refresh Workload Identity access tokens for the pinned remote API only.
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use http::{header, HeaderValue, Request, Uri};
use kube::client::Body;
use serde::Deserialize;
use tokio::sync::Mutex;
use tokio::time::Instant;
use tower::{buffer::Buffer, BoxError, Layer, Service, ServiceExt};

use super::unavailable;
use crate::{AdapterError, Result};

const ENDPOINT: &str =
    "http://metadata.google.internal/computeMetadata/v1/instance/service-accounts/default/token";
const RESPONSE_LIMIT: usize = 64 * 1024;
const REFRESH_MARGIN: u64 = 60;

mod transport;

pub(super) fn client(config: kube::Config) -> Result<kube::Client> {
    let layer = MetadataLayer {
        target: config.cluster_url.clone(),
        tokens: Arc::new(MetadataTokens::new(ENDPOINT, Duration::from_secs(5))?),
    };
    transport::client(config, &layer)
}

struct CachedToken {
    authorization: HeaderValue,
    refresh_at: Instant,
}
struct MetadataTokens {
    endpoint: String,
    client: reqwest::Client,
    cache: Mutex<Option<CachedToken>>,
    timeout: Duration,
}
#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: u64,
    token_type: String,
}
impl MetadataTokens {
    fn new(endpoint: &str, timeout: Duration) -> Result<Self> {
        Ok(Self {
            endpoint: endpoint.into(),
            client: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(timeout)
                .connect_timeout(timeout)
                .build()
                .map_err(|_| token_unavailable())?,
            cache: Mutex::new(None),
            timeout,
        })
    }
    async fn authorization(&self) -> Result<HeaderValue> {
        tokio::time::timeout(self.timeout, self.cached_authorization())
            .await
            .map_err(|_| token_unavailable())?
    }
    async fn cached_authorization(&self) -> Result<HeaderValue> {
        let mut cache = self.cache.lock().await;
        let started = Instant::now();
        if let Some(cached) = cache.as_ref().filter(|cached| started < cached.refresh_at) {
            return Ok(cached.authorization.clone());
        }
        // A failed refresh never reuses a stale token or another credential source.
        *cache = None;
        let mut response = self
            .client
            .get(&self.endpoint)
            .header("Metadata-Flavor", "Google")
            .send()
            .await
            .map_err(|_| token_unavailable())?;
        if !response.status().is_success()
            || response
                .headers()
                .get("Metadata-Flavor")
                .is_none_or(|value| value != "Google")
            || response
                .content_length()
                .is_some_and(|size| size > RESPONSE_LIMIT as u64)
        {
            return Err(token_unavailable());
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| token_unavailable())? {
            if bytes.len().saturating_add(chunk.len()) > RESPONSE_LIMIT {
                return Err(token_unavailable());
            }
            bytes.extend_from_slice(&chunk);
        }
        let token: TokenResponse =
            serde_json::from_slice(&bytes).map_err(|_| token_unavailable())?;
        if token.token_type != "Bearer"
            || !(REFRESH_MARGIN + 1..=86400).contains(&token.expires_in)
            || token.access_token.is_empty()
            || token.access_token.len() > 16384
            || token
                .access_token
                .bytes()
                .any(|c| c.is_ascii_whitespace() || c.is_ascii_control())
        {
            return Err(token_unavailable());
        }
        let mut authorization = HeaderValue::from_str(&format!("Bearer {}", token.access_token))
            .map_err(|_| token_unavailable())?;
        authorization.set_sensitive(true);
        let refresh_at = started + Duration::from_secs(token.expires_in - REFRESH_MARGIN);
        if Instant::now() >= refresh_at {
            return Err(token_unavailable());
        }
        *cache = Some(CachedToken {
            authorization: authorization.clone(),
            refresh_at,
        });
        Ok(authorization)
    }
}
fn token_unavailable() -> AdapterError {
    unavailable("Google metadata access token is unavailable")
}

#[derive(Clone)]
struct MetadataLayer {
    target: Uri,
    tokens: Arc<MetadataTokens>,
}
impl<S> Layer<S> for MetadataLayer
where
    S: Service<Request<Body>> + Send + 'static,
    S::Future: Send + 'static,
    S::Error: Into<BoxError> + Send + Sync,
    S::Response: Send + 'static,
{
    type Service = MetadataService<Buffer<Request<Body>, S::Future>>;
    fn layer(&self, inner: S) -> Self::Service {
        MetadataService {
            inner: Buffer::new(inner, 64),
            target: self.target.clone(),
            tokens: self.tokens.clone(),
        }
    }
}
#[derive(Clone)]
struct MetadataService<S> {
    inner: S,
    target: Uri,
    tokens: Arc<MetadataTokens>,
}
impl<S> Service<Request<Body>> for MetadataService<S>
where
    S: Service<Request<Body>> + Clone + Send + 'static,
    S::Future: Send + 'static,
    S::Error: Into<BoxError>,
    S::Response: Send + 'static,
{
    type Response = S::Response;
    type Error = BoxError;
    type Future =
        Pin<Box<dyn Future<Output = std::result::Result<Self::Response, BoxError>> + Send>>;
    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<std::result::Result<(), BoxError>> {
        Poll::Ready(Ok(()))
    }
    fn call(&mut self, mut request: Request<Body>) -> Self::Future {
        let inner = self.inner.clone();
        let tokens = self.tokens.clone();
        let target = self.target.clone();
        Box::pin(async move {
            if request
                .uri()
                .authority()
                .is_some_and(|host| Some(host) != target.authority())
                || request
                    .uri()
                    .scheme()
                    .is_some_and(|scheme| Some(scheme) != target.scheme())
                || request.headers().get(header::HOST).is_some_and(|host| {
                    target
                        .authority()
                        .is_none_or(|authority| host.as_bytes() != authority.as_str().as_bytes())
                })
            {
                return Err(
                    Box::new(unavailable("native Kubernetes request target changed")) as BoxError,
                );
            }
            request
                .headers_mut()
                .insert(header::AUTHORIZATION, tokens.authorization().await?);
            inner.oneshot(request).await.map_err(Into::into)
        })
    }
}

#[cfg(test)]
mod tests;
