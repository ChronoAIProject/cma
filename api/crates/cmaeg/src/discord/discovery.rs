//! Read-only Discord bot discovery. Credentials never leave the fixed API origin.
use super::{protocol::endpoint, DiscordRest, GatewayError, RestError};
use cmaeg_core::channel::Secret;
use reqwest::{Client, Url};
use serde::Deserialize;
use std::time::Duration;

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionLimit {
    pub total: u64,
    pub remaining: u64,
    pub reset_after: u64,
    pub max_concurrency: u64,
}
#[derive(Deserialize)]
pub struct Discovery {
    pub url: String,
    pub shards: u64,
    pub session_start_limit: SessionLimit,
}
#[derive(Clone)]
pub struct DiscordApi {
    client: Client,
    origin: Url,
    limits: DiscordRest,
    pub(super) thread_metadata: bool,
}
impl DiscordApi {
    pub fn new() -> Result<Self, GatewayError> {
        Ok(Self {
            client: Client::builder()
                .connect_timeout(Duration::from_secs(2))
                .timeout(Duration::from_secs(4))
                .redirect(reqwest::redirect::Policy::none())
                .no_proxy()
                .build()
                .map_err(|_| GatewayError::Unavailable)?,
            origin: Url::parse("https://discord.com/api/v10/")
                .map_err(|_| GatewayError::InvalidConfiguration)?,
            limits: DiscordRest::new().map_err(|_| GatewayError::Unavailable)?,
            thread_metadata: false,
        })
    }
    pub(super) fn with_thread_metadata(mut self, enabled: bool) -> Self {
        self.thread_metadata = enabled;
        self
    }
    pub(super) fn retry_after(&self, token: &Secret) -> Duration {
        match self.limits.ready(token) {
            Err(RestError::RateLimited(seconds)) => Duration::from_secs(seconds),
            _ => Duration::ZERO,
        }
    }
    pub async fn discover(
        &self,
        token: &Secret,
        expected_bot: &str,
    ) -> Result<Discovery, GatewayError> {
        #[derive(Deserialize)]
        struct User {
            id: String,
            #[serde(default)]
            bot: bool,
        }
        let user: User = self.get("users/@me", token).await?;
        if !user.bot || user.id != expected_bot {
            return Err(GatewayError::Authority);
        }
        let discovery: Discovery = self.get("gateway/bot", token).await?;
        endpoint(&discovery.url)?;
        let limit = &discovery.session_start_limit;
        // This transport is deliberately one shard. Discord may require sharding
        // for larger bots; refusing is safer than missing guilds silently.
        if discovery.shards != 1
            || limit.total == 0
            || limit.remaining > limit.total
            || limit.max_concurrency == 0
            || limit.reset_after == 0
            || limit.reset_after > 7 * 86_400_000
        {
            return Err(GatewayError::InvalidConfiguration);
        }
        Ok(discovery)
    }
    pub(super) async fn get<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        token: &Secret,
    ) -> Result<T, GatewayError> {
        self.limits.ready(token).map_err(|error| match error {
            RestError::RateLimited(_) => GatewayError::RateLimited,
            _ => GatewayError::Unavailable,
        })?;
        let url = self
            .origin
            .join(path)
            .map_err(|_| GatewayError::InvalidConfiguration)?;
        let mut response = self
            .client
            .get(url)
            .header("Authorization", format!("Bot {}", token.as_str()))
            .send()
            .await
            .map_err(|_| GatewayError::Unavailable)?;
        let status = response.status().as_u16();
        let retry = response
            .headers()
            .get("retry-after")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<f64>().ok())
            .filter(|value| value.is_finite() && *value > 0.0);
        if response
            .headers()
            .get("x-ratelimit-remaining")
            .is_some_and(|value| value.as_bytes() == b"0")
        {
            let reset = response
                .headers()
                .get("x-ratelimit-reset-after")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<f64>().ok())
                .unwrap_or(1.0);
            self.limits.cooldown(token, reset.max(retry.unwrap_or(0.0)));
        }
        if status == 429 {
            self.limits.cooldown(token, retry.unwrap_or(60.0));
        }
        if matches!(status, 401 | 403) {
            return Err(GatewayError::Authority);
        }
        if status != 429 && !response.status().is_success() {
            return Err(GatewayError::Unavailable);
        }
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| GatewayError::Unavailable)?
        {
            if body.len() + chunk.len() > 65_536 {
                return Err(GatewayError::Protocol);
            }
            body.extend_from_slice(&chunk);
        }
        if status == 429 {
            let body_retry = serde_json::from_slice::<serde_json::Value>(&body)
                .ok()
                .and_then(|value| value["retry_after"].as_f64())
                .filter(|value| value.is_finite() && *value > 0.0);
            self.limits
                .cooldown(token, body_retry.unwrap_or(60.0).max(retry.unwrap_or(0.0)));
            return Err(GatewayError::RateLimited);
        }
        serde_json::from_slice(&body).map_err(|_| GatewayError::Protocol)
    }
    #[cfg(test)]
    pub(super) fn fixture(origin: &str) -> Self {
        let mut result = Self::new().unwrap();
        result.origin = Url::parse(origin).unwrap();
        result.limits = DiscordRest::fixture(origin);
        result
    }
}
