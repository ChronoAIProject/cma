//! NyxID OAuth client credentials, confined to one process and one HTTPS origin.
use super::{ProducerScope, ProducerTokenError};
use super::{ProducerToken, ProducerTokenProvider};
use async_trait::async_trait;
use serde::Deserialize;
use std::{
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{sync::Mutex, time::Instant};
use zeroize::Zeroizing;

pub(crate) struct ClientCredentialsProvider {
    http: reqwest::Client,
    endpoint: reqwest::Url,
    client_id: Zeroizing<String>,
    client_secret: Zeroizing<String>,
    cache: Mutex<Cache>,
    scope: ProducerScope,
}
#[derive(Default)]
struct Cache {
    token: Option<CachedToken>,
    failure: Option<(Instant, ProducerTokenError)>,
}
struct CachedToken {
    value: ProducerToken,
    refresh_at: Instant,
    expires_at: u64,
}
#[derive(Clone, Copy)]
struct Failure {
    category: &'static str,
    retryable: bool,
}
impl Failure {
    fn temporary(category: &'static str) -> Self {
        Self {
            category,
            retryable: true,
        }
    }
    fn invalid(category: &'static str) -> Self {
        Self {
            category,
            retryable: false,
        }
    }
    fn token_error(self) -> ProducerTokenError {
        let detail = format!("producer token {}", self.category);
        if self.retryable {
            ProducerTokenError::Retryable(detail)
        } else {
            ProducerTokenError::Invalid(detail)
        }
    }
}
impl ClientCredentialsProvider {
    pub(crate) fn new(
        base_url: &reqwest::Url,
        client_id: String,
        client_secret: String,
        scope: ProducerScope,
    ) -> Result<Arc<Self>, String> {
        let endpoint = base_url
            .join("/oauth/token")
            .map_err(|_| "invalid producer token origin")?;
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(2))
            .connect_timeout(Duration::from_secs(2))
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .user_agent("CMA-Sandbox-Funding/1")
            .build()
            .map_err(|_| "cannot construct producer token client")?;
        Ok(Arc::new(Self {
            http,
            endpoint,
            client_id: Zeroizing::new(client_id),
            client_secret: Zeroizing::new(client_secret),
            cache: Mutex::new(Cache::default()),
            scope,
        }))
    }
    async fn get(
        &self,
        rejected: Option<&ProducerToken>,
    ) -> Result<ProducerToken, ProducerTokenError> {
        // Hold the async mutex across exchange/backoff: waiters share success AND failure.
        let mut cache = self.cache.lock().await;
        let previous_expiry = cache.token.as_ref().map(|token| token.expires_at);
        if let Some(token) = &cache.token {
            let rejected_current = rejected.is_some_and(|old| Arc::ptr_eq(&old.0, &token.value.0));
            if !rejected_current && Instant::now() < token.refresh_at {
                return Ok(token.value.clone());
            }
            if rejected_current {
                cache.token = None;
            }
        }
        if let Some((retry_at, error)) = &cache.failure {
            if Instant::now() < *retry_at {
                return Err(error.clone());
            }
        }
        // Cancellation must not cause waiting callers to stampede the endpoint.
        cache.failure = Some((
            Instant::now() + Duration::from_secs(1),
            ProducerTokenError::Retryable("producer token refresh interrupted".into()),
        ));
        for attempt in 0..3 {
            match self.exchange().await {
                Ok(token) => {
                    tracing::info!(
                        category = "oauth_client_credentials",
                        outcome = "success",
                        expires_at = token.expires_at,
                        "Sandbox producer token refresh"
                    );
                    let value = token.value.clone();
                    cache.token = Some(token);
                    cache.failure = None;
                    return Ok(value);
                }
                Err(error) if error.retryable && attempt < 2 => {
                    // Jitter prevents replicas with similar lifetimes from retrying in lockstep.
                    let jitter = u64::from(uuid::Uuid::new_v4().as_bytes()[0]) % 50;
                    tokio::time::sleep(Duration::from_millis((100 << attempt) + jitter)).await;
                }
                Err(error) => {
                    tracing::warn!(
                        category = error.category,
                        outcome = "failure",
                        expires_at = previous_expiry,
                        "Sandbox producer token refresh"
                    );
                    let error = error.token_error();
                    cache.failure = Some((Instant::now() + Duration::from_secs(1), error.clone()));
                    return Err(error);
                }
            }
        }
        unreachable!("bounded exchange always returns on its final attempt")
    }
    async fn exchange(&self) -> Result<CachedToken, Failure> {
        let start = Instant::now();
        let wall_start = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let mut response = self
            .http
            .post(self.endpoint.clone())
            .form(&[
                ("grant_type", "client_credentials"),
                ("client_id", self.client_id.as_str()),
                ("client_secret", self.client_secret.as_str()),
                ("scope", self.scope.as_str()),
            ])
            .send()
            .await
            .map_err(|_| Failure::temporary("transport"))?;
        let status = response.status();
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
            return Err(Failure::temporary("endpoint_unavailable"));
        }
        if status != reqwest::StatusCode::OK {
            return Err(Failure::invalid("endpoint_rejected"));
        }
        let mut bytes = Zeroizing::new(Vec::new());
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| Failure::temporary("transport"))?
        {
            if bytes.len().saturating_add(chunk.len()) > 32768 {
                return Err(Failure::invalid("response_too_large"));
            }
            bytes.extend_from_slice(&chunk);
        }
        #[derive(Deserialize)]
        struct Response {
            access_token: String,
            token_type: String,
            expires_in: u64,
        }
        let token: Response =
            serde_json::from_slice(&bytes).map_err(|_| Failure::invalid("malformed_response"))?;
        let value = ProducerToken::new(token.access_token);
        if !token.token_type.eq_ignore_ascii_case("bearer")
            || !(1..=31_536_000).contains(&token.expires_in)
            || value.bearer().is_empty()
            || value.bearer().len() > 16384
            || !value
                .bearer()
                .bytes()
                .all(|byte| (33..=126).contains(&byte))
        {
            return Err(Failure::invalid("malformed_response"));
        }
        let lifetime = Duration::from_secs(token.expires_in);
        // Short tokens cannot reserve five minutes; retain 20% of their lifetime.
        let margin = if lifetime > Duration::from_secs(300) {
            (lifetime / 5).max(Duration::from_secs(300))
        } else {
            lifetime / 5
        };
        let refresh_at = start + lifetime - margin;
        if Instant::now() >= refresh_at {
            return Err(Failure::invalid("lifetime_too_short"));
        }
        Ok(CachedToken {
            value,
            refresh_at,
            expires_at: wall_start + token.expires_in,
        })
    }
}
#[async_trait]
impl ProducerTokenProvider for ClientCredentialsProvider {
    async fn token(&self) -> Result<ProducerToken, ProducerTokenError> {
        self.get(None).await
    }
    async fn rejected(
        &self,
        token: &ProducerToken,
    ) -> Result<Option<ProducerToken>, ProducerTokenError> {
        self.get(Some(token)).await.map(Some)
    }
}
#[cfg(test)]
pub(crate) mod tests;
