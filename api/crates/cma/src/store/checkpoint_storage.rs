//! The existing platform-proxied chrono-storage object contract.

use std::{sync::Arc, time::Duration};

use futures_util::{stream, StreamExt, TryStreamExt};
use layer3_adapter::hosted::{CheckpointArchiveDownload, CheckpointArchiveUpload};
use layer3_adapter::{AdapterError, Result};

use crate::config::CheckpointStorageConfig;

#[cfg(test)]
mod memory;
mod multipart;
mod part_transfer;
#[cfg(test)]
mod tests;
mod token;
mod transfer;

pub(super) const OBJECT_HTTP_TIMEOUT: Duration = Duration::from_secs(180);
pub(super) const UPLOAD_LIFETIME: Duration = Duration::from_secs(1_200);

#[async_trait::async_trait]
pub(crate) trait CheckpointBlobStore: Send + Sync {
    async fn upload(&self, key: &str, archive: CheckpointArchiveUpload) -> Result<()>;
    async fn download(
        &self,
        key: &str,
        reference: &str,
        sha256: &str,
        size: u64,
    ) -> Result<CheckpointArchiveDownload>;
    #[allow(dead_code)]
    async fn delete(&self, key: &str) -> Result<()>;

    /// True only when storage durably rejects later writes to every owned key.
    /// Observed absence alone cannot settle an uncertain upload.
    #[allow(dead_code)]
    async fn finalize_delete(&self, key: &str) -> Result<bool> {
        self.delete(key).await?;
        Ok(false)
    }
}

#[derive(Clone)]
pub(crate) struct ChronoStorage {
    objects: reqwest::Url,
    download: reqwest::Url,
    tokens: Arc<token::StorageToken>,
    http: reqwest::Client,
    download_parts: Arc<tokio::sync::Semaphore>,
}

impl ChronoStorage {
    pub(crate) fn new(
        platform_origin: &str,
        config: &CheckpointStorageConfig,
    ) -> Result<Arc<Self>> {
        let origin = reqwest::Url::parse(platform_origin)
            .map_err(|_| unavailable("checkpoint storage origin is invalid"))?;
        if origin.scheme() != "https" || origin.origin().ascii_serialization() != platform_origin {
            return Err(unavailable(
                "checkpoint storage requires a canonical HTTPS platform origin",
            ));
        }
        Self::from_origin(origin, config).map(Arc::new)
    }

    fn from_origin(origin: reqwest::Url, config: &CheckpointStorageConfig) -> Result<Self> {
        let mut objects = origin.clone();
        objects
            .path_segments_mut()
            .map_err(|_| unavailable("checkpoint storage origin is invalid"))?
            .extend([
                "api",
                "v1",
                "proxy",
                "s",
                "chrono-storage-service",
                "api",
                "buckets",
                &config.bucket,
                "objects",
            ]);
        let mut download = objects.clone();
        download
            .path_segments_mut()
            .map_err(|_| unavailable("checkpoint storage endpoint is invalid"))?
            .push("download");
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .timeout(OBJECT_HTTP_TIMEOUT)
            .build()
            .map_err(|_| unavailable("checkpoint storage HTTP client is unavailable"))?;
        Ok(Self {
            objects,
            download,
            tokens: Arc::new(token::StorageToken::new(origin, config)?),
            http,
            download_parts: Arc::new(tokio::sync::Semaphore::new(multipart::DOWNLOAD_CONCURRENCY)),
        })
    }
}

#[async_trait::async_trait]
impl CheckpointBlobStore for ChronoStorage {
    async fn upload(&self, key: &str, archive: CheckpointArchiveUpload) -> Result<()> {
        if let Some(size) = multipart::size_from_key(key)? {
            multipart::upload(self, key, size, archive).await
        } else {
            transfer::upload(self, key, archive).await
        }
    }

    async fn download(
        &self,
        key: &str,
        reference: &str,
        sha256: &str,
        size: u64,
    ) -> Result<CheckpointArchiveDownload> {
        if let Some(recorded_size) = multipart::size_from_key(key)? {
            if recorded_size != size {
                return Err(rejected("checkpoint part layout has a different size"));
            }
            multipart::download(self, key, reference, sha256, size).await
        } else {
            transfer::download(self, key, reference, sha256, size).await
        }
    }

    async fn delete(&self, key: &str) -> Result<()> {
        if let Some(size) = multipart::size_from_key(key)? {
            // The durable key names every possible part, even when upload never published a manifest.
            stream::iter(0..size.div_ceil(multipart::PART_BYTES as u64))
                .map(|index| async move {
                    self.delete_object(&multipart::part_key(key, index)).await
                })
                .buffer_unordered(4)
                .try_collect::<Vec<_>>()
                .await?;
        }
        self.delete_object(key).await
    }

    async fn finalize_delete(&self, key: &str) -> Result<bool> {
        if let Some(size) = multipart::size_from_key(key)? {
            stream::iter(0..size.div_ceil(multipart::PART_BYTES as u64))
                .map(|index| async move {
                    self.fence_deleted_object(&multipart::part_key(key, index))
                        .await
                })
                .buffer_unordered(4)
                .try_collect::<Vec<_>>()
                .await?;
        }
        self.fence_deleted_object(key).await?;
        Ok(true)
    }
}

impl ChronoStorage {
    #[allow(dead_code)]
    async fn fence_deleted_object(&self, key: &str) -> Result<()> {
        // CMA allocates immutable UUID object keys. Generation 2 permanently
        // supersedes their legacy writes (generation 0) and fenced writes (1).
        // Query transport survives the NyxID proxy's header filtering.
        let response = self
            .http
            .delete(self.objects.clone())
            .query(&[("key", key), ("mutationGeneration", "2")])
            .bearer_auth(self.tokens.token().await?)
            .send()
            .await
            .map_err(|_| unavailable("checkpoint deletion fence is unconfirmed"))?;
        if !response.status().is_success() {
            return Err(unavailable("checkpoint deletion fence was rejected"));
        }
        let mut bytes = Vec::new();
        let mut body = response.bytes_stream();
        while let Some(chunk) = body.next().await {
            let chunk =
                chunk.map_err(|_| unavailable("checkpoint deletion receipt was interrupted"))?;
            if bytes.len().saturating_add(chunk.len()) > 16 * 1024 {
                return Err(rejected("checkpoint deletion receipt exceeds its bound"));
            }
            bytes.extend_from_slice(&chunk);
        }
        let receipt: serde_json::Value = serde_json::from_slice(&bytes)
            .map_err(|_| unavailable("checkpoint deletion receipt is invalid"))?;
        let data = &receipt["data"];
        if !receipt.get("error").is_some_and(serde_json::Value::is_null)
            || data["deleted"] != true
            || data["generation"] != 2
            || data["fenceVersion"] != 1
        {
            return Err(unavailable(
                "checkpoint storage did not confirm a durable deletion fence",
            ));
        }
        Ok(())
    }

    #[allow(dead_code)]
    async fn delete_object(&self, key: &str) -> Result<()> {
        let response = self
            .http
            .delete(self.objects.clone())
            .query(&[("key", key)])
            .bearer_auth(self.tokens.token().await?)
            .send()
            .await;
        let absent = self
            .http
            .get(self.download.clone())
            .query(&[("key", key)])
            .bearer_auth(self.tokens.token().await?)
            .send()
            .await
            .map_err(|_| unavailable("checkpoint storage delete verification failed"))?;
        if absent.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(());
        }
        if response.is_err() {
            return Err(unavailable("checkpoint storage deletion is uncertain"));
        }
        Err(unavailable(
            "checkpoint storage object remains after deletion",
        ))
    }
}

pub(super) fn checkpoint_object_key(base: String, size: u64) -> String {
    if size > multipart::PART_BYTES as u64 {
        format!("{base}.parts-v1-{size}")
    } else {
        base
    }
}

#[allow(dead_code)]
pub(super) fn checkpoint_object_key_base(key: &str) -> Result<&str> {
    if multipart::size_from_key(key)?.is_some() {
        Ok(key
            .rsplit_once(".parts-v1-")
            .expect("validated part layout")
            .0)
    } else {
        Ok(key)
    }
}

fn unavailable(message: &'static str) -> AdapterError {
    AdapterError::Unavailable(message.into())
}

fn rejected(message: &'static str) -> AdapterError {
    AdapterError::Rejected(message.into())
}

#[cfg(test)]
pub(crate) use memory::test_storage;
