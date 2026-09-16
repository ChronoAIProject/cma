//! Fixed origins and bounded read-only requests using the existing workload token.
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use reqwest::{Client, Response};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::{unavailable, Result};

pub(super) struct Transport {
    client: Client,
    compute: String,
    metadata: String,
}

pub(super) struct ReadReply {
    pub status: u16,
    pub value: Value,
    pub body: Vec<u8>,
    pub url: String,
    pub observation_id: String,
}

impl Transport {
    pub(super) fn new() -> Result<Self> {
        Self::origins(
            "https://compute.googleapis.com/compute/v1/".into(),
            "http://metadata.google.internal/computeMetadata/v1/instance/service-accounts/default/token".into(),
        )
    }

    fn origins(compute: String, metadata: String) -> Result<Self> {
        let client = Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|_| unavailable("backing observer HTTP client is unavailable"))?;
        Ok(Self {
            client,
            compute,
            metadata,
        })
    }

    #[cfg(test)]
    pub(super) fn fixture(origin: &str) -> Result<Self> {
        Self::origins(format!("{origin}/compute/v1/"), format!("{origin}/token"))
    }

    pub(super) async fn read(&self, path: &str) -> Result<ReadReply> {
        tokio::time::timeout(Duration::from_secs(15), self.read_inner(path))
            .await
            .map_err(|_| unavailable("backing observation exceeded its deadline"))?
    }

    async fn read_inner(&self, path: &str) -> Result<ReadReply> {
        let token_response = self
            .client
            .get(&self.metadata)
            .header("Metadata-Flavor", "Google")
            .send()
            .await
            .map_err(|_| unavailable("workload read authority is unavailable"))?;
        if token_response.status().as_u16() != 200
            || token_response
                .headers()
                .get("metadata-flavor")
                .and_then(|v| v.to_str().ok())
                != Some("Google")
        {
            return Err(unavailable("workload read authority is unavailable"));
        }
        let (_, token_value) = bounded_json(token_response, 16_384).await?;
        let token = token_value["access_token"]
            .as_str()
            .filter(|token| {
                !token.is_empty()
                    && token.len() <= 8_192
                    && token.bytes().all(|c| c.is_ascii_graphic())
            })
            .ok_or_else(|| unavailable("workload read authority is unavailable"))?;
        if token_value["token_type"] != "Bearer"
            || !token_value["expires_in"]
                .as_u64()
                .is_some_and(|expiry| (1..=86_400).contains(&expiry))
        {
            return Err(unavailable("workload read authority is unavailable"));
        }
        let url = format!("{}{path}", self.compute);
        let observation_id = observation_id(&url);
        let response = self
            .client
            .get(&url)
            .bearer_auth(token)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|_| unavailable("exact backing disk read is unavailable"))?;
        let status = response.status().as_u16();
        if !matches!(status, 200 | 404) {
            return Err(unavailable(
                "exact backing disk read was not authorized or conclusive",
            ));
        }
        let (body, value) = bounded_json(response, 65_536).await?;
        Ok(ReadReply {
            status,
            value,
            body,
            url,
            observation_id,
        })
    }
}

async fn bounded_json(mut response: Response, limit: usize) -> Result<(Vec<u8>, Value)> {
    if response
        .content_length()
        .is_some_and(|size| size > limit as u64)
    {
        return Err(unavailable("backing observer response exceeds its bound"));
    }
    let mut bytes = Vec::with_capacity(limit);
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| unavailable("backing observer response is incomplete"))?
    {
        if chunk.len() > limit.saturating_sub(bytes.len()) {
            return Err(unavailable("backing observer response exceeds its bound"));
        }
        bytes.extend_from_slice(&chunk);
    }
    let value = serde_json::from_slice(&bytes)
        .map_err(|_| unavailable("backing observer response is not valid JSON"))?;
    Ok((bytes, value))
}

fn observation_id(url: &str) -> String {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let mut hash = Sha256::new();
    hash.update(b"cma-gke-pd-read-v1\0");
    hash.update(url.as_bytes());
    hash.update(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            .to_be_bytes(),
    );
    hash.update(std::process::id().to_be_bytes());
    hash.update(SEQUENCE.fetch_add(1, Ordering::Relaxed).to_be_bytes());
    format!("cma-pd-read-{:x}", hash.finalize())
}
