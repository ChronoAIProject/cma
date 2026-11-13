use super::tests::{settle, Store};
use super::*;
use layer2_core::document_artifacts::*;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Mutex,
};

struct Purge {
    calls: AtomicUsize,
    backlog_calls: AtomicUsize,
    position: Mutex<Option<PurgePosition>>,
}
impl Purge {
    fn document(&self) -> PurgeClaimable {
        PurgeClaimable {
            id: format!("doc_{:032x}", 1),
            owner_hash: digest(b"worker-owner"),
            deleted_at: 0,
            last_reserved_at: None,
            pass: 1,
            next_at: 0,
            position: self.position.lock().unwrap().clone(),
        }
    }
}
#[async_trait::async_trait]
impl DocumentPurgeStore for Purge {
    async fn due(&self, _: i64, _: usize) -> Result<Vec<PurgeClaimable>, DocumentError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(vec![self.document()])
    }
    async fn claim(
        &self,
        _: &str,
        holder: &str,
        now: i64,
    ) -> Result<Option<PurgeLease>, DocumentError> {
        Ok(Some(PurgeLease {
            document: self.document(),
            holder: holder.into(),
            fence: 1,
            expires_at: now + 90_000,
        }))
    }
    async fn objects(&self, lease: &PurgeLease) -> Result<Vec<String>, DocumentError> {
        Ok((0..100)
            .map(|n| format!("{}{:064x}.md", lease.document.prefix(), n))
            .collect())
    }
    async fn advance(&self, _: &PurgeLease, step: PurgeStep) -> Result<bool, DocumentError> {
        if let PurgeStep::Progress { position, .. } = step {
            *self.position.lock().unwrap() = Some(position);
        }
        Ok(true)
    }
    async fn release(&self, _: &PurgeLease) -> Result<bool, DocumentError> {
        Ok(false)
    }
    async fn block(&self, _: &PurgeLease, _: &str) -> Result<bool, DocumentError> {
        Ok(false)
    }
    async fn backlog(&self, _: i64) -> Result<PurgeBacklog, DocumentError> {
        self.backlog_calls.fetch_add(1, Ordering::SeqCst);
        Ok(PurgeBacklog::default())
    }
}

#[tokio::test(start_paused = true)]
async fn document_purge_worker_revisits_progress_promptly_and_keeps_reclaim_fair() {
    let reclaim = Arc::new(Store(AtomicUsize::new(0)));
    let purge = Arc::new(Purge {
        calls: AtomicUsize::new(0),
        backlog_calls: AtomicUsize::new(0),
        position: Mutex::new(None),
    });
    let maintenance = DocumentMaintenance::new(
        reclaim.clone(),
        Arc::new(memory::MemoryDocumentBlobs::default()),
        true,
    )
    .with_purge(purge.clone(), true);
    let mut worker =
        DocumentMaintenanceWorker::new(Some(maintenance)).with_purge(purge.clone(), true);
    worker.budget = crate::background::BackgroundBudget::new();
    let (stop, signal) = tokio::sync::watch::channel(false);
    let task = worker.spawn(signal);
    settle().await;
    tokio::time::advance(Duration::from_secs(30)).await;
    settle().await;
    assert_eq!(purge.calls.load(Ordering::SeqCst), 1);
    assert_eq!(reclaim.0.load(Ordering::SeqCst), 1);
    tokio::time::advance(Duration::from_secs(1)).await;
    settle().await;
    assert!(
        purge.calls.load(Ordering::SeqCst) > 1,
        "durable waves must resume before a five-minute reclaim interval"
    );
    tokio::time::advance(Duration::from_secs(300)).await;
    settle().await;
    assert!(
        reclaim.0.load(Ordering::SeqCst) >= 2,
        "continuous purge must not starve reclaim"
    );
    stop.send(true).unwrap();
    task.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn document_purge_worker_observes_unsupported_backlog_without_claiming_or_blob_io() {
    let purge = Arc::new(Purge {
        calls: AtomicUsize::new(0),
        backlog_calls: AtomicUsize::new(0),
        position: Mutex::new(None),
    });
    let mut worker = DocumentMaintenanceWorker::new(None).with_purge(purge.clone(), false);
    worker.budget = crate::background::BackgroundBudget::new();
    let (stop, signal) = tokio::sync::watch::channel(false);
    let task = worker.spawn(signal);
    settle().await;
    tokio::time::advance(Duration::from_secs(30)).await;
    settle().await;
    assert_eq!(purge.calls.load(Ordering::SeqCst), 0);
    assert_eq!(purge.backlog_calls.load(Ordering::SeqCst), 1);
    stop.send(true).unwrap();
    task.await.unwrap();
}
