use std::sync::Arc;

use tokio::sync::mpsc;

/// Maximum archive accepted by the runtime contract: header, manifest, payload.
pub const MAX_CHECKPOINT_ARCHIVE_BYTES: u64 = 24 + (128 * 1024 * 1024) + (16 * 1024 * 1024 * 1024);

/// A runtime archive being committed to durable storage.
pub struct CheckpointArchiveUpload {
    pub object_reference: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub chunks: mpsc::Receiver<crate::Result<Vec<u8>>>,
}

/// A durable archive being streamed back toward a runtime.
pub struct CheckpointArchiveDownload {
    pub object_reference: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub chunks: mpsc::Receiver<crate::Result<Vec<u8>>>,
}

/// Proof returned only after an archive is durably committed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckpointArchiveReceipt {
    pub object_reference: String,
    pub sha256: String,
    pub size_bytes: u64,
}

/// Durable streaming authority for whole-home checkpoint archives.
#[async_trait::async_trait]
pub trait CheckpointArchiveStore: Send + Sync {
    async fn upload(
        &self,
        archive: CheckpointArchiveUpload,
    ) -> crate::Result<CheckpointArchiveReceipt>;

    async fn download(&self, object_reference: &str) -> crate::Result<CheckpointArchiveDownload>;
}

/// Produces an archive authority already confined to one validated sandbox.
pub trait CheckpointArchiveStoreFactory: Send + Sync {
    fn for_sandbox(&self, sandbox_id: &str) -> crate::Result<Arc<dyn CheckpointArchiveStore>>;
}
