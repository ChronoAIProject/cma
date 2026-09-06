//! The existing platform-proxied chrono-storage object contract.

use std::{sync::Arc, time::Duration};

use layer3_adapter::hosted::{CheckpointArchiveDownload, CheckpointArchiveUpload};
use layer3_adapter::{AdapterError, Result};

use crate::config::CheckpointStorageConfig;

#[cfg(test)]
mod memory;
#[cfg(test)]
mod tests;
mod token;
mod transfer;

pub(super) const OBJECT_HTTP_TIMEOUT: Duration = Duration::from_secs(180);
pub(super) const UPLOAD_LIFETIME: Duration = Duration::from_secs(600);

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
    async fn delete(&self, key: &str) -> Result<()>;
}

pub(crate) struct ChronoStorage {
    objects: reqwest::Url,
    download: reqwest::Url,
    tokens: token::StorageToken,
    http: reqwest::Client,
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
            tokens: token::StorageToken::new(origin, config)?,
            http,
        })
    }
}

#[async_trait::async_trait]
impl CheckpointBlobStore for ChronoStorage {
    async fn upload(&self, key: &str, archive: CheckpointArchiveUpload) -> Result<()> {
        transfer::upload(self, key, archive).await
    }

    async fn download(
        &self,
        key: &str,
        reference: &str,
        sha256: &str,
        size: u64,
    ) -> Result<CheckpointArchiveDownload> {
        transfer::download(self, key, reference, sha256, size).await
    }

    async fn delete(&self, key: &str) -> Result<()> {
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

fn unavailable(message: &'static str) -> AdapterError {
    AdapterError::Unavailable(message.into())
}

fn rejected(message: &'static str) -> AdapterError {
    AdapterError::Rejected(message.into())
}

#[cfg(test)]
pub(crate) use memory::test_storage;
