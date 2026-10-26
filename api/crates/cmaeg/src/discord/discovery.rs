//! Read-only Discord bot discovery. Credentials never leave the fixed API origin.
use super::{protocol::endpoint, GatewayError};
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
}
impl DiscordApi {
    pub fn new() -> Result<Self, GatewayError> {
        Ok(Self {
            client: Client::builder()
                .connect_timeout(Duration::from_secs(2))
                .timeout(Duration::from_secs(4))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|_| GatewayError::Unavailable)?,
            origin: Url::parse("https://discord.com/api/v10/")
                .map_err(|_| GatewayError::InvalidConfiguration)?,
        })
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
    async fn get<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        token: &Secret,
    ) -> Result<T, GatewayError> {
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
        if matches!(response.status().as_u16(), 401 | 403) {
            return Err(GatewayError::Authority);
        }
        if !response.status().is_success() {
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
        serde_json::from_slice(&body).map_err(|_| GatewayError::Protocol)
    }
    #[cfg(test)]
    pub(super) fn fixture(origin: &str) -> Self {
        let mut result = Self::new().unwrap();
        result.origin = Url::parse(origin).unwrap();
        result
    }
}
