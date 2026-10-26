//! NyxID OAuth endpoint discovery. The console is enabled only once NyxID's
//! OpenID configuration advertises everything sign-in relies on, and only on
//! the configured NyxID origin, so a misconfigured or impersonated provider
//! can never receive a code, a verifier or the client secret.
use std::{
    sync::{Arc, RwLock},
    time::Duration,
};

use reqwest::{Client, StatusCode, Url};
use serde_json::Value;

/// A successful document is fetched again after this long, so an endpoint
/// change reaches the console without a restart.
pub(crate) const REFRESH_INTERVAL: Duration = Duration::from_secs(60 * 60);
const FIRST_RETRY: Duration = Duration::from_secs(30);
const MAX_RETRY: Duration = Duration::from_secs(5 * 60);
const FETCH_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_DOCUMENT_BYTES: usize = 64 * 1024;

/// The endpoints sign-in uses, each verified to be on the NyxID origin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OidcEndpoints {
    pub authorization: Url,
    pub token: Url,
    pub revocation: Url,
}

/// The endpoints in use, shared by sign-in, refresh and revocation. Empty
/// until discovery succeeds.
#[derive(Default)]
pub struct EndpointCell(RwLock<Option<Arc<OidcEndpoints>>>);

impl EndpointCell {
    pub fn get(&self) -> Option<Arc<OidcEndpoints>> {
        self.0
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub(crate) fn set(&self, endpoints: Option<Arc<OidcEndpoints>>) {
        *self
            .0
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = endpoints;
    }
}

/// Why discovery did not produce endpoints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiscoveryError {
    /// NyxID could not be read; the last good endpoints stay in use.
    Unavailable,
    /// NyxID answered with a document the console must not use; the console
    /// is disabled until a later document passes.
    Mismatch(&'static str),
}

/// Fetch and validate `{nyxid}/.well-known/openid-configuration`.
pub(crate) async fn fetch(
    client: &Client,
    nyxid_base_url: &str,
) -> Result<OidcEndpoints, DiscoveryError> {
    let origin = Url::parse(nyxid_base_url).map_err(|_| DiscoveryError::Mismatch("origin"))?;
    let url = origin
        .join("/.well-known/openid-configuration")
        .map_err(|_| DiscoveryError::Mismatch("origin"))?;
    let mut response = client
        .get(url)
        .timeout(FETCH_TIMEOUT)
        .send()
        .await
        .map_err(|_| DiscoveryError::Unavailable)?;
    if response.status() != StatusCode::OK {
        return Err(DiscoveryError::Unavailable);
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| DiscoveryError::Unavailable)?
    {
        if body.len() + chunk.len() > MAX_DOCUMENT_BYTES {
            return Err(DiscoveryError::Unavailable);
        }
        body.extend_from_slice(&chunk);
    }
    let document: Value = serde_json::from_slice(&body).map_err(|_| DiscoveryError::Unavailable)?;
    validate(&document, &origin)
}

/// Every condition the console needs: PKCE S256, `client_secret_post`, and
/// authorization, token and revocation endpoints on the NyxID origin.
pub(crate) fn validate(document: &Value, origin: &Url) -> Result<OidcEndpoints, DiscoveryError> {
    if !lists(document, "code_challenge_methods_supported", "S256") {
        return Err(DiscoveryError::Mismatch("code_challenge_method"));
    }
    if !lists(
        document,
        "token_endpoint_auth_methods_supported",
        "client_secret_post",
    ) {
        return Err(DiscoveryError::Mismatch("token_endpoint_auth_method"));
    }
    Ok(OidcEndpoints {
        authorization: endpoint(document, "authorization_endpoint", origin)?,
        token: endpoint(document, "token_endpoint", origin)?,
        revocation: endpoint(document, "revocation_endpoint", origin)?,
    })
}

fn lists(document: &Value, field: &str, wanted: &str) -> bool {
    document
        .get(field)
        .and_then(Value::as_array)
        .is_some_and(|values| values.iter().any(|value| value.as_str() == Some(wanted)))
}

/// An HTTPS URL on the configured NyxID origin, without credentials or a
/// fragment. Plain HTTP passes only when the configured origin is itself a
/// loopback HTTP origin, which only local harnesses use.
fn endpoint(document: &Value, field: &'static str, origin: &Url) -> Result<Url, DiscoveryError> {
    let url = document
        .get(field)
        .and_then(Value::as_str)
        .and_then(|value| Url::parse(value).ok())
        .ok_or(DiscoveryError::Mismatch(field))?;
    let secure = url.scheme() == "https" || loopback(&url);
    if !secure
        || url.origin() != origin.origin()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(DiscoveryError::Mismatch(field));
    }
    Ok(url)
}

fn loopback(url: &Url) -> bool {
    url.scheme() == "http"
        && match url.host() {
            Some(url::Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
            Some(url::Host::Ipv4(address)) => address.is_loopback(),
            Some(url::Host::Ipv6(address)) => address.is_loopback(),
            None => false,
        }
}

/// The wait after `failures` consecutive failed attempts: 30 s, doubling to
/// at most 5 minutes.
pub(crate) fn retry_delay(failures: u32) -> Duration {
    let doublings = failures.saturating_sub(1).min(8);
    FIRST_RETRY.saturating_mul(1 << doublings).min(MAX_RETRY)
}

/// Where discovery results go: the console publishes endpoints to its
/// requests and its enablement gauge.
pub(crate) trait DiscoverySink: Send + Sync {
    fn publish(&self, endpoints: Option<Arc<OidcEndpoints>>);
    fn current(&self) -> bool;
}

/// Discover forever: a success is kept for [`REFRESH_INTERVAL`]; a failure is
/// retried with [`retry_delay`]. An unreachable NyxID keeps the last good
/// endpoints, but a document that fails validation disables the console,
/// because NyxID itself then says sign-in would not be safe.
pub(crate) async fn run<F, Fut>(sink: Arc<dyn DiscoverySink>, mut attempt: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<OidcEndpoints, DiscoveryError>>,
{
    let mut failures = 0_u32;
    loop {
        let wait = match attempt().await {
            Ok(endpoints) => {
                if failures > 0 || !sink.current() {
                    tracing::info!(outcome = "ok", "console discovery");
                }
                failures = 0;
                sink.publish(Some(Arc::new(endpoints)));
                REFRESH_INTERVAL
            }
            Err(error) => {
                failures = failures.saturating_add(1);
                let wait = retry_delay(failures);
                match error {
                    DiscoveryError::Unavailable => tracing::warn!(
                        outcome = "unavailable",
                        retry_in_s = wait.as_secs(),
                        "console discovery failed; NyxID is unreachable"
                    ),
                    DiscoveryError::Mismatch(reason) => {
                        tracing::error!(
                            outcome = "mismatch",
                            reason,
                            retry_in_s = wait.as_secs(),
                            "console discovery refused NyxID's OpenID configuration; the console stays disabled"
                        );
                        sink.publish(None);
                    }
                }
                wait
            }
        };
        tokio::time::sleep(wait).await;
    }
}

#[cfg(test)]
#[path = "oidc_tests.rs"]
mod tests;
