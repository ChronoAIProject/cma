//! Renewable credentials shared by CMA records and confined provider management.
use cmaeg_core::channel::Secret;
use serde::Deserialize;
use std::{sync::Arc, time::Duration};
use tokio::{sync::Mutex, time::Instant};

#[derive(Debug, Clone)]
pub(crate) enum ServiceAuth {
    Static(Secret),
    Account(Arc<Account>),
}

#[derive(Debug)]
pub(crate) struct Account {
    http: reqwest::Client,
    endpoint: reqwest::Url,
    credentials: crate::config::ServiceAccountConfig,
    cache: Mutex<Cache>,
}

#[derive(Debug, Default)]
struct Cache {
    token: Option<CachedToken>,
    retry_at: Option<Instant>,
}

#[derive(Debug)]
struct CachedToken {
    value: Secret,
    refresh_at: Instant,
    expires_at: Instant,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ServiceAuthUnavailable;
impl std::fmt::Display for ServiceAuthUnavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CMA service-account authentication is unavailable")
    }
}
impl std::error::Error for ServiceAuthUnavailable {}

impl ServiceAuth {
    pub(crate) fn from_config(config: &crate::Config) -> Result<Self, ServiceAuthUnavailable> {
        match &config.cma_service_account {
            None => Ok(Self::Static(config.management.cma_service_token.clone())),
            Some(credentials) => {
                Self::account(&config.management.nyxid_base_url, credentials.clone())
            }
        }
    }

    pub(crate) fn account(
        origin: &str,
        credentials: crate::config::ServiceAccountConfig,
    ) -> Result<Self, ServiceAuthUnavailable> {
        let origin = crate::config::validate_origin("identity provider", origin)
            .map_err(|_| ServiceAuthUnavailable)?;
        let endpoint = reqwest::Url::parse(&format!("{origin}/oauth/token"))
            .map_err(|_| ServiceAuthUnavailable)?;
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(2))
            .timeout(Duration::from_secs(5))
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .user_agent(format!("cmaeg/{}", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|_| ServiceAuthUnavailable)?;
        Ok(Self::Account(Arc::new(Account {
            http,
            endpoint,
            credentials,
            cache: Mutex::new(Cache::default()),
        })))
    }

    pub(crate) async fn bearer(&self) -> Result<Secret, ServiceAuthUnavailable> {
        match self {
            Self::Static(token) => Ok(token.clone()),
            Self::Account(account) => {
                // Bound callers waiting behind a refresh as well as the exchange.
                tokio::time::timeout(Duration::from_secs(6), account.bearer())
                    .await
                    .map_err(|_| ServiceAuthUnavailable)?
            }
        }
    }
}

impl Account {
    async fn bearer(&self) -> Result<Secret, ServiceAuthUnavailable> {
        let mut cache = self.cache.lock().await;
        let now = Instant::now();
        if let Some(token) = &cache.token {
            if now < token.refresh_at {
                return Ok(token.value.clone());
            }
        }
        if cache.retry_at.is_some_and(|retry| now < retry) {
            return usable(&cache);
        }
        // Also bounds retry storms when an outer request cancels token exchange.
        cache.retry_at = Some(now + Duration::from_secs(2));
        match self.exchange().await {
            Ok(token) => {
                let result = token.value.clone();
                cache.token = Some(token);
                cache.retry_at = None;
                Ok(result)
            }
            Err(error) => {
                cache.retry_at = Some(Instant::now() + Duration::from_secs(2));
                usable(&cache).map_err(|_| error)
            }
        }
    }

    async fn exchange(&self) -> Result<CachedToken, ServiceAuthUnavailable> {
        let start = Instant::now();
        let mut response = self
            .http
            .post(self.endpoint.clone())
            .form(&[
                ("grant_type", "client_credentials"),
                ("client_id", self.credentials.client_id.as_str()),
                ("client_secret", self.credentials.client_secret.as_str()),
                ("scope", "openid roles"),
            ])
            .send()
            .await
            .map_err(|_| ServiceAuthUnavailable)?;
        if response.status() != reqwest::StatusCode::OK
            || response.content_length().is_some_and(|bytes| bytes > 32768)
        {
            return Err(ServiceAuthUnavailable);
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| ServiceAuthUnavailable)? {
            if bytes.len().saturating_add(chunk.len()) > 32768 {
                return Err(ServiceAuthUnavailable);
            }
            bytes.extend_from_slice(&chunk);
        }
        #[derive(Deserialize)]
        struct Token {
            access_token: Secret,
            token_type: String,
            expires_in: u64,
        }
        let token: Token = serde_json::from_slice(&bytes).map_err(|_| ServiceAuthUnavailable)?;
        if !token.token_type.eq_ignore_ascii_case("bearer")
            || !(1..=31_536_000).contains(&token.expires_in)
            || token.access_token.as_str().is_empty()
            || token.access_token.as_str().len() > 16384
            || !token
                .access_token
                .as_str()
                .bytes()
                .all(|byte| (33..=126).contains(&byte))
        {
            return Err(ServiceAuthUnavailable);
        }
        let lifetime = Duration::from_secs(token.expires_in);
        let expires_at = start + lifetime;
        if Instant::now() >= expires_at {
            return Err(ServiceAuthUnavailable);
        }
        let margin = (lifetime / 5).min(Duration::from_secs(60));
        Ok(CachedToken {
            value: token.access_token,
            refresh_at: expires_at - margin,
            expires_at,
        })
    }
}

fn usable(cache: &Cache) -> Result<Secret, ServiceAuthUnavailable> {
    cache
        .token
        .as_ref()
        .filter(|token| Instant::now() < token.expires_at)
        .map(|token| token.value.clone())
        .ok_or(ServiceAuthUnavailable)
}

#[cfg(test)]
mod tests;
