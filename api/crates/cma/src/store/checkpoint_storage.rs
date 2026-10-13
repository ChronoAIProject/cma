//! Retired CMA archive boundary and pure decoding of historical object keys.
//! Production has no storage credentials, HTTP client, or object mutation authority.

use std::time::Duration;

use layer3_adapter::hosted::{
    CheckpointArchiveDownload, CheckpointArchiveUpload, MAX_CHECKPOINT_ARCHIVE_BYTES,
};
use layer3_adapter::{AdapterError, Result};

#[cfg(test)]
mod memory;
#[cfg(test)]
mod tests;

const PART_BYTES: u64 = 32 * 1024 * 1024;
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

/// Keeps historical manifest decoding available without granting any blob access.
pub(crate) struct RetiredCheckpointStorage;

#[async_trait::async_trait]
impl CheckpointBlobStore for RetiredCheckpointStorage {
    async fn upload(&self, _key: &str, _archive: CheckpointArchiveUpload) -> Result<()> {
        Err(retired())
    }

    async fn download(
        &self,
        _key: &str,
        _reference: &str,
        _sha256: &str,
        _size: u64,
    ) -> Result<CheckpointArchiveDownload> {
        Err(retired())
    }

    async fn delete(&self, _key: &str) -> Result<()> {
        // Do not claim historical objects were removed or allow manifests to be pruned.
        Err(retired())
    }
}

fn retired() -> AdapterError {
    AdapterError::Unsupported(
        "CMA checkpoint storage is retired; Sandbox state belongs to its persistent volume".into(),
    )
}

pub(super) fn checkpoint_object_key(base: String, size: u64) -> String {
    if size > PART_BYTES {
        format!("{base}.parts-v1-{size}")
    } else {
        base
    }
}

fn size_from_key(key: &str) -> Result<Option<u64>> {
    let Some((_, value)) = key.rsplit_once(".parts-v1-") else {
        return Ok(None);
    };
    value
        .parse::<u64>()
        .ok()
        .filter(|size| *size > PART_BYTES && *size <= MAX_CHECKPOINT_ARCHIVE_BYTES)
        .filter(|size| size.to_string() == value)
        .map(Some)
        .ok_or_else(|| rejected("checkpoint part layout is invalid"))
}

#[allow(dead_code)]
pub(super) fn checkpoint_object_key_base(key: &str) -> Result<&str> {
    if size_from_key(key)?.is_some() {
        Ok(key
            .rsplit_once(".parts-v1-")
            .expect("validated part layout")
            .0)
    } else {
        Ok(key)
    }
}

#[cfg(test)]
fn unavailable(message: &'static str) -> AdapterError {
    AdapterError::Unavailable(message.into())
}

fn rejected(message: &'static str) -> AdapterError {
    AdapterError::Rejected(message.into())
}

#[cfg(test)]
pub(crate) use memory::test_storage;
