use std::time::{Duration, Instant};

use futures_util::StreamExt;
use tokio::sync::Mutex;

use super::{unavailable, CheckpointStorageConfig, Result};

struct Cached {
    token: String,
    expires: Instant,
}

pub(super) struct StorageToken {
    endpoint: reqwest::Url,
    client_id: String,
    client_secret: String,
    cached: Mutex<Option<Cached>>,
    http: reqwest::Client,
}

impl StorageToken {
    pub(super) fn new(mut origin: reqwest::Url, config: &CheckpointStorageConfig) -> Result<Self> {
        origin.set_path("/oauth/token");
        Ok(Self {
            endpoint: origin,
            client_id: config.client_id.clone(),
            client_secret: config.client_secret.clone(),
            cached: Mutex::new(None),
            http: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(30))
                .build()
                .map_err(|_| unavailable("checkpoint storage token client is unavailable"))?,
        })
    }

    pub(super) async fn token(&self) -> Result<String> {
        let mut cached = self.cached.lock().await;
        if let Some(value) = cached
            .as_ref()
            .filter(|value| Instant::now() < value.expires)
        {
            return Ok(value.token.clone());
        }
        let started = Instant::now();
        let response = self
            .http
            .post(self.endpoint.clone())
            .form(&[
                ("grant_type", "client_credentials"),
                ("client_id", self.client_id.as_str()),
                ("client_secret", self.client_secret.as_str()),
            ])
            .send()
            .await
            .map_err(|_| unavailable("checkpoint storage token request failed"))?;
        if !response.status().is_success()
            || response.content_length().is_some_and(|size| size > 32_768)
        {
            return Err(unavailable(
                "checkpoint storage token response was rejected",
            ));
        }
        let mut bytes = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(part) = stream.next().await {
            let part = part.map_err(|_| unavailable("checkpoint storage token response failed"))?;
            if bytes.len().saturating_add(part.len()) > 32_768 {
                return Err(unavailable(
                    "checkpoint storage token response exceeds its bound",
                ));
            }
            bytes.extend_from_slice(&part);
        }
        #[derive(serde::Deserialize)]
        struct TokenResponse {
            access_token: String,
            expires_in: Option<u64>,
        }
        let token: TokenResponse = serde_json::from_slice(&bytes)
            .map_err(|_| unavailable("checkpoint storage token response is malformed"))?;
        let lifetime = token.expires_in.unwrap_or(900);
        if !(61..=86_400).contains(&lifetime)
            || token.access_token.trim().is_empty()
            || reqwest::header::HeaderValue::from_str(&token.access_token).is_err()
        {
            return Err(unavailable("checkpoint storage token response is unusable"));
        }
        let expires = started + Duration::from_secs(lifetime - 60);
        if expires <= Instant::now() {
            return Err(unavailable(
                "checkpoint storage token expired during issuance",
            ));
        }
        *cached = Some(Cached {
            token: token.access_token.clone(),
            expires,
        });
        Ok(token.access_token)
    }
}
