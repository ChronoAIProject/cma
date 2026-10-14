use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};

use sha2::{Digest, Sha256};
use tokio::sync::mpsc;

use super::*;

#[derive(Default)]
struct MemoryStorage(Mutex<BTreeMap<String, Vec<u8>>>);

pub(crate) fn test_storage(database: &str) -> Arc<dyn CheckpointBlobStore> {
    static STORES: OnceLock<Mutex<BTreeMap<String, Arc<MemoryStorage>>>> = OnceLock::new();
    STORES
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .entry(database.into())
        .or_default()
        .clone()
}

#[async_trait::async_trait]
impl CheckpointBlobStore for MemoryStorage {
    async fn upload(&self, key: &str, mut archive: CheckpointArchiveUpload) -> Result<()> {
        let mut bytes = Vec::new();
        while let Some(chunk) = archive.chunks.recv().await {
            bytes.extend(chunk?);
            if bytes.len() as u64 > archive.size_bytes {
                return Err(rejected("checkpoint upload size mismatch"));
            }
        }
        if bytes.len() as u64 != archive.size_bytes
            || format!("{:x}", Sha256::digest(&bytes)) != archive.sha256
        {
            return Err(rejected("checkpoint upload integrity mismatch"));
        }
        self.0.lock().unwrap().insert(key.into(), bytes);
        Ok(())
    }

    async fn download(
        &self,
        key: &str,
        reference: &str,
        sha256: &str,
        size: u64,
    ) -> Result<CheckpointArchiveDownload> {
        let bytes = self
            .0
            .lock()
            .unwrap()
            .get(key)
            .cloned()
            .ok_or_else(|| unavailable("checkpoint object missing"))?;
        let (sender, chunks) = mpsc::channel(2);
        let valid = size == bytes.len() as u64 && format!("{:x}", Sha256::digest(&bytes)) == sha256;
        tokio::spawn(async move {
            for chunk in bytes.chunks(256 * 1024) {
                if sender.send(Ok(chunk.to_vec())).await.is_err() {
                    return;
                }
            }
            if !valid {
                let _ = sender
                    .send(Err(unavailable("checkpoint download integrity mismatch")))
                    .await;
            }
        });
        Ok(CheckpointArchiveDownload {
            object_reference: reference.into(),
            sha256: sha256.into(),
            size_bytes: size,
            chunks,
        })
    }

    async fn delete(&self, key: &str) -> Result<()> {
        self.0.lock().unwrap().remove(key);
        Ok(())
    }
}
