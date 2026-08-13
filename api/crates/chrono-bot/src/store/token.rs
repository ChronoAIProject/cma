//! Obtaining the credential this service presents to the identity platform.
//!
//! A machine identity's token is short-lived by design, so it cannot be read
//! once from configuration and held. It is minted on demand and cached until
//! shortly before it expires.
//!
//! Kept behind a trait so the thing that stores session context does not know
//! how its credential was obtained. That is what lets a test run without a
//! token endpoint, and what would let a different identity scheme replace this
//! one without touching the store.

use std::sync::Arc;
use std::time::{Duration, Instant};

use layer2_core::store::StoreError;
use tokio::sync::Mutex;

/// Renew this long before expiry.
///
/// A token that is valid when checked but expires mid-flight is
/// indistinguishable from a wrong credential at the far end, so the margin is
/// wide enough to cover a slow upload rather than merely the round trip.
const RENEW_MARGIN: Duration = Duration::from_secs(60);

/// Assumed lifetime when the provider does not state one.
///
/// Deliberately short: re-minting needlessly costs one request, while treating
/// a short token as long-lived produces failures well after the cause.
const ASSUMED_LIFETIME: Duration = Duration::from_secs(900);

/// Something that can supply a bearer credential.
#[async_trait::async_trait]
pub trait TokenSource: Send + Sync {
    /// A credential that is valid now.
    async fn token(&self) -> Result<String, StoreError>;
}

/// A credential held verbatim.
///
/// Test-only. Every deployment obtains a token from the platform, so a fixed
/// credential exists purely to exercise the store without a token endpoint;
/// compiling it into the binary would offer a way to bypass that.
#[cfg(test)]
pub struct StaticToken(pub String);

#[cfg(test)]
#[async_trait::async_trait]
impl TokenSource for StaticToken {
    async fn token(&self) -> Result<String, StoreError> {
        Ok(self.0.clone())
    }
}

/// What was minted, and when it stops being usable.
struct Cached {
    token: String,
    expires_at: Instant,
}

/// A machine identity that mints its own tokens.
///
/// Exchanges a client id and secret for an access token through the platform's
/// token endpoint, which is how a service acts as itself rather than on behalf
/// of a signed-in person.
pub struct ServiceAccountToken {
    token_url: String,
    client_id: String,
    client_secret: String,
    cached: Mutex<Option<Cached>>,
    http: reqwest::Client,
}

impl ServiceAccountToken {
    /// Build a token source over a platform base URL.
    pub fn new(
        platform_base_url: &str,
        client_id: impl Into<String>,
        client_secret: impl Into<String>,
    ) -> Self {
        Self {
            token_url: format!("{}/oauth/token", platform_base_url.trim_end_matches('/')),
            client_id: client_id.into(),
            client_secret: client_secret.into(),
            cached: Mutex::new(None),
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .expect("http client builds"),
        }
    }

    async fn mint(&self) -> Result<Cached, StoreError> {
        let requested_at = Instant::now();
        let resp = self
            .http
            .post(&self.token_url)
            .form(&[
                ("grant_type", "client_credentials"),
                ("client_id", self.client_id.as_str()),
                ("client_secret", self.client_secret.as_str()),
            ])
            .send()
            .await
            .map_err(|e| StoreError::Unavailable(format!("token request: {e}")))?;

        let status = resp.status();
        if !status.is_success() {
            // The body can carry the provider's reason, but it is also the one
            // place a credential could be echoed back, so it is not logged.
            return Err(StoreError::Unavailable(format!(
                "token endpoint returned {status}"
            )));
        }

        #[derive(serde::Deserialize)]
        struct TokenResponse {
            access_token: String,
            expires_in: Option<u64>,
        }
        let body: TokenResponse = resp
            .json()
            .await
            .map_err(|e| StoreError::Malformed(format!("token response: {e}")))?;
        if body.access_token.trim().is_empty() {
            return Err(StoreError::Malformed("token response was empty".into()));
        }

        let lifetime = body
            .expires_in
            .map(Duration::from_secs)
            .unwrap_or(ASSUMED_LIFETIME);
        // Measured from before the request, so time spent waiting counts
        // against the lifetime rather than being credited to it.
        let expires_at = requested_at + lifetime.saturating_sub(RENEW_MARGIN);
        tracing::info!(
            lifetime_secs = lifetime.as_secs(),
            "minted a platform access token"
        );
        Ok(Cached {
            token: body.access_token,
            expires_at,
        })
    }
}

#[async_trait::async_trait]
impl TokenSource for ServiceAccountToken {
    async fn token(&self) -> Result<String, StoreError> {
        // The lock is held across the mint so a burst of callers produces one
        // token rather than one each; the alternative wastes tokens and can
        // trip the provider's rate limit.
        let mut guard = self.cached.lock().await;
        if let Some(c) = guard.as_ref() {
            if Instant::now() < c.expires_at {
                return Ok(c.token.clone());
            }
        }
        let fresh = self.mint().await?;
        let token = fresh.token.clone();
        *guard = Some(fresh);
        Ok(token)
    }
}

/// Build the token source a deployment's configuration describes.
///
/// Returns `None` when the platform identity is not configured, which leaves
/// context storage switched off rather than half-configured.
pub fn from_config(
    platform_base_url: &str,
    client_id: &str,
    client_secret: &str,
) -> Option<Arc<dyn TokenSource>> {
    if platform_base_url.trim().is_empty() {
        return None;
    }
    if client_id.trim().is_empty() || client_secret.trim().is_empty() {
        return None;
    }
    Some(Arc::new(ServiceAccountToken::new(
        platform_base_url,
        client_id.trim(),
        client_secret.trim(),
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_static_credential_is_returned_unchanged() {
        assert_eq!(StaticToken("abc".into()).token().await.unwrap(), "abc");
    }

    #[test]
    fn the_token_endpoint_hangs_off_the_platform_root() {
        let s = ServiceAccountToken::new("https://platform.example", "id", "secret");
        assert_eq!(s.token_url, "https://platform.example/oauth/token");
    }

    #[test]
    fn a_trailing_slash_does_not_double_up() {
        let s = ServiceAccountToken::new("https://platform.example/", "id", "secret");
        assert_eq!(s.token_url, "https://platform.example/oauth/token");
    }

    #[tokio::test]
    async fn an_unreachable_endpoint_is_reported_rather_than_cached() {
        // Port 1 is reserved and never listening.
        let s = ServiceAccountToken::new("http://127.0.0.1:1", "id", "secret");
        assert!(matches!(s.token().await, Err(StoreError::Unavailable(_))));
        // A failed mint must leave nothing behind, or a later call would serve
        // a token that was never issued.
        assert!(s.cached.lock().await.is_none());
    }

    #[test]
    fn an_unconfigured_identity_yields_no_token_source() {
        // Each field missing on its own must switch storage off rather than
        // build something that fails on first use.
        assert!(from_config("", "id", "secret").is_none());
        assert!(from_config("https://p", "", "secret").is_none());
        assert!(from_config("https://p", "id", "").is_none());
        assert!(from_config("https://p", "  ", "secret").is_none());
    }

    #[test]
    fn a_fully_configured_identity_yields_a_token_source() {
        assert!(from_config("https://p", "id", "secret").is_some());
    }

    #[test]
    fn the_renewal_margin_is_subtracted_from_the_stated_lifetime() {
        // A token treated as valid until the instant it expires will be
        // rejected mid-request; the margin is what prevents that.
        let lifetime = Duration::from_secs(3600);
        assert_eq!(
            lifetime.saturating_sub(RENEW_MARGIN),
            Duration::from_secs(3540)
        );
    }

    #[test]
    fn a_lifetime_shorter_than_the_margin_does_not_wrap() {
        // saturating_sub matters: a very short token must expire immediately
        // rather than underflow into an effectively infinite one.
        assert_eq!(
            Duration::from_secs(30).saturating_sub(RENEW_MARGIN),
            Duration::ZERO
        );
    }
}
