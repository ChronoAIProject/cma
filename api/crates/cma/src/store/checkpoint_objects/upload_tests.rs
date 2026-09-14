use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use layer3_adapter::hosted::CheckpointArchiveDownload;
use tokio::sync::mpsc;

use super::*;
use crate::store::checkpoint_objects::model::{manifest_document, read_manifest, sha256_hex};

const BYTES: &[u8] = b"bounded delayed checkpoint upload";

struct DelayedStore {
    inner: Arc<dyn CheckpointBlobStore>,
    delay: Duration,
    calls: AtomicUsize,
    cancelled: AtomicBool,
}

impl DelayedStore {
    fn new(delay: u64) -> Self {
        Self {
            inner: crate::store::checkpoint_storage::test_storage(
                &uuid::Uuid::new_v4().to_string(),
            ),
            delay: Duration::from_secs(delay),
            calls: AtomicUsize::new(0),
            cancelled: AtomicBool::new(false),
        }
    }
}

struct InFlight<'a> {
    cancelled: &'a AtomicBool,
    completed: bool,
}

impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        if !self.completed {
            self.cancelled.store(true, Ordering::SeqCst);
        }
    }
}

#[async_trait::async_trait]
impl CheckpointBlobStore for DelayedStore {
    async fn upload(&self, key: &str, archive: CheckpointArchiveUpload) -> AdapterResult<()> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let mut flight = InFlight {
            cancelled: &self.cancelled,
            completed: false,
        };
        tokio::time::sleep(self.delay).await;
        self.inner.upload(key, archive).await?;
        flight.completed = true;
        Ok(())
    }

    async fn download(
        &self,
        key: &str,
        reference: &str,
        sha256: &str,
        size: u64,
    ) -> AdapterResult<CheckpointArchiveDownload> {
        self.inner.download(key, reference, sha256, size).await
    }

    async fn delete(&self, key: &str) -> AdapterResult<()> {
        self.inner.delete(key).await
    }
}

fn stage() -> (ManifestDoc, SystemTime) {
    let now = DateTime::from_millis(1_700_000_000_000);
    let sha256 = sha256_hex(BYTES);
    let identity =
        ArchiveIdentity::from_input(format!("home-{sha256}.cma"), sha256, BYTES.len() as u64)
            .unwrap();
    (
        ManifestDoc::new("sbx_upload_deadline", &identity, now),
        now.to_system_time(),
    )
}

fn archive(stage: &ManifestDoc) -> CheckpointArchiveUpload {
    let (sender, chunks) = mpsc::channel(1);
    sender.try_send(Ok(BYTES.to_vec())).unwrap();
    CheckpointArchiveUpload {
        object_reference: stage.object_reference.clone(),
        sha256: stage.sha256.clone(),
        size_bytes: stage.size_bytes as u64,
        chunks,
    }
}

fn expired(error: layer3_adapter::AdapterError) {
    assert!(
        matches!(error, layer3_adapter::AdapterError::Unavailable(detail) if detail == "checkpoint archive upload lifetime expired")
    );
}

#[tokio::test(start_paused = true)]
async fn staged_transfer_completes_after_the_old_six_hundred_second_budget() {
    let (stage, now) = stage();
    let store = DelayedStore::new(601);
    let started = tokio::time::Instant::now();
    transfer_before_deadline(&store, &stage, archive(&stage), now)
        .await
        .unwrap();
    assert_eq!(started.elapsed(), Duration::from_secs(601));
    assert_eq!(store.calls.load(Ordering::SeqCst), 1);
    assert!(!store.cancelled.load(Ordering::SeqCst));
    let mut download = store
        .download(
            &stage.object_key,
            &stage.object_reference,
            &stage.sha256,
            stage.size_bytes as u64,
        )
        .await
        .unwrap();
    assert_eq!(download.chunks.recv().await.unwrap().unwrap(), BYTES);
    assert!(download.chunks.recv().await.is_none());
}

#[tokio::test(start_paused = true)]
async fn staged_transfer_cancels_at_exactly_twelve_hundred_seconds() {
    let (stage, now) = stage();
    let store = DelayedStore::new(1_201);
    let started = tokio::time::Instant::now();
    expired(
        transfer_before_deadline(&store, &stage, archive(&stage), now)
            .await
            .unwrap_err(),
    );
    assert_eq!(started.elapsed(), Duration::from_secs(1_200));
    assert_eq!(store.calls.load(Ordering::SeqCst), 1);
    assert!(store.cancelled.load(Ordering::SeqCst));
    assert!(store
        .download(
            &stage.object_key,
            &stage.object_reference,
            &stage.sha256,
            stage.size_bytes as u64
        )
        .await
        .is_err());
}

#[tokio::test(start_paused = true)]
async fn reopened_legacy_stage_keeps_its_recorded_six_hundred_second_deadline() {
    let (mut stage, now) = stage();
    stage.cleanup_after = DateTime::from_system_time(now + Duration::from_secs(600));
    let stage = read_manifest(manifest_document(&stage).unwrap()).unwrap();
    assert_eq!(
        stage.cleanup_after.to_system_time(),
        now + Duration::from_secs(600)
    );
    let store = DelayedStore::new(601);
    let started = tokio::time::Instant::now();
    expired(
        transfer_before_deadline(&store, &stage, archive(&stage), now)
            .await
            .unwrap_err(),
    );
    assert_eq!(started.elapsed(), Duration::from_secs(600));
    assert!(store.cancelled.load(Ordering::SeqCst));
}

#[tokio::test(start_paused = true)]
async fn staged_transfer_uses_only_remaining_time_and_never_restarts_an_expired_stage() {
    let (stage, now) = stage();
    let store = DelayedStore::new(2);
    let started = tokio::time::Instant::now();
    expired(
        transfer_before_deadline(
            &store,
            &stage,
            archive(&stage),
            now + Duration::from_secs(1_199),
        )
        .await
        .unwrap_err(),
    );
    assert_eq!(started.elapsed(), Duration::from_secs(1));
    assert!(store.cancelled.load(Ordering::SeqCst));

    let store = DelayedStore::new(0);
    expired(
        transfer_before_deadline(
            &store,
            &stage,
            archive(&stage),
            now + Duration::from_secs(1_201),
        )
        .await
        .unwrap_err(),
    );
    assert_eq!(store.calls.load(Ordering::SeqCst), 0);
}
