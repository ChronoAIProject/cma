use super::*;
use layer2_core::document_artifacts::*;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

pub(super) struct Store(pub(super) AtomicUsize);
#[async_trait::async_trait]
impl DocumentReclaimStore for Store {
    async fn ready(&self) -> Result<bool, DocumentError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(true)
    }
    async fn candidates(&self, _: i64, _: usize) -> Result<Vec<ObjectCandidate>, DocumentError> {
        Ok(vec![])
    }
    async fn judge(&self, _: &ObjectCandidate) -> Result<Judgement, DocumentError> {
        unreachable!()
    }
    async fn claim_delete(
        &self,
        _: &ObjectCandidate,
        _: &str,
        _: i64,
    ) -> Result<Option<DeleteClaim>, DocumentError> {
        unreachable!()
    }
    async fn finalize(&self, _: &DeleteClaim) -> Result<bool, DocumentError> {
        unreachable!()
    }
    async fn backlog(&self, _: i64) -> Result<ReclaimBacklog, DocumentError> {
        Ok(ReclaimBacklog::default())
    }
}
pub(super) async fn settle() {
    for _ in 0..12 {
        tokio::task::yield_now().await;
    }
}
fn worker(store: Arc<Store>) -> DocumentMaintenanceWorker {
    DocumentMaintenanceWorker::new(Some(DocumentMaintenance::new(
        store,
        Arc::new(memory::MemoryDocumentBlobs::default()),
        true,
    )))
}
#[tokio::test(start_paused = true)]
async fn document_reclaim_worker_waits_for_startup_and_honours_shutdown() {
    let store = Arc::new(Store(AtomicUsize::new(0)));
    let (stop, signal) = tokio::sync::watch::channel(false);
    let task = worker(store.clone()).spawn(signal);
    settle().await;
    tokio::time::advance(std::time::Duration::from_secs(29)).await;
    settle().await;
    assert_eq!(store.0.load(Ordering::SeqCst), 0);
    tokio::time::advance(std::time::Duration::from_secs(1)).await;
    settle().await;
    assert_eq!(store.0.load(Ordering::SeqCst), 1);
    stop.send(true).unwrap();
    task.await.unwrap();
}
#[tokio::test(start_paused = true)]
async fn document_reclaim_worker_without_storage_stays_optional_and_idle() {
    let activity = crate::worker_supervision::Workers::new().get("document_maintenance");
    assert!(!activity.snapshot().required);
    assert!(activity.snapshot().available());
    let (stop, signal) = tokio::sync::watch::channel(false);
    let task = DocumentMaintenanceWorker::new(None).spawn_supervised(signal, activity.clone());
    settle().await;
    tokio::time::advance(RECLAIM_INTERVAL * 2).await;
    settle().await;
    assert_eq!(activity.snapshot().state, "running");
    assert!(!task.is_finished());
    stop.send(true).unwrap();
    task.await.unwrap();
    assert_eq!(activity.snapshot().state, "stopped");
}

#[tokio::test(start_paused = true)]
async fn document_reclaim_supervision_preserves_child_and_pacing_after_scheduler_panic() {
    let store = Arc::new(Store(AtomicUsize::new(0)));
    let activity = crate::worker_supervision::Workers::new().get("document_maintenance");
    let (stop, signal) = tokio::sync::watch::channel(false);
    let task = worker(store.clone()).spawn_supervised(signal, activity.clone());
    settle().await;
    tokio::time::advance(std::time::Duration::from_secs(30)).await;
    settle().await;
    assert_eq!(store.0.load(Ordering::SeqCst), 1);
    crate::worker_supervision::panic_next_poll(&activity);
    tokio::time::advance(std::time::Duration::from_secs(1)).await;
    settle().await;
    assert_eq!(activity.snapshot().state, "backoff");
    assert!(activity.snapshot().available());
    tokio::time::advance(std::time::Duration::from_secs(1)).await;
    settle().await;
    assert_eq!(activity.snapshot().state, "running");
    tokio::time::advance(std::time::Duration::from_secs(30)).await;
    settle().await;
    assert_eq!(store.0.load(Ordering::SeqCst), 1);
    tokio::time::advance(std::time::Duration::from_secs(268)).await;
    settle().await;
    assert_eq!(store.0.load(Ordering::SeqCst), 2);
    stop.send(true).unwrap();
    task.await.unwrap();
}
#[test]
fn document_reclaim_worker_retry_delay_is_at_least_the_interval() {
    assert_eq!(next_delay(None), RECLAIM_INTERVAL);
    assert_eq!(
        next_delay(Some(std::time::Duration::from_secs(900))),
        std::time::Duration::from_secs(900)
    );
    assert_eq!(
        next_delay(Some(std::time::Duration::from_secs(1))),
        RECLAIM_INTERVAL
    );
}

#[tokio::test(start_paused = true)]
async fn document_reclaim_worker_skips_denied_budget_until_next_round() {
    let budget = crate::background::BackgroundBudget::new();
    let hold = |name| {
        let work = crate::background::BackgroundWork::new(
            budget.clone(),
            name,
            std::time::Duration::from_secs(3600),
            std::time::Duration::ZERO,
        );
        tokio::spawn(async move {
            work.run(|| {
                std::future::pending::<Result<(), layer2_core::sandbox::store::SandboxStoreError>>()
            })
            .await
        })
    };
    let first = hold("document_reclaim_test_first");
    let second = hold("document_reclaim_test_second");
    settle().await;
    let store = Arc::new(Store(AtomicUsize::new(0)));
    let mut worker = worker(store.clone());
    worker.budget = budget;
    let (stop, signal) = tokio::sync::watch::channel(false);
    let task = worker.spawn(signal);
    settle().await;
    tokio::time::advance(std::time::Duration::from_secs(30)).await;
    settle().await;
    assert_eq!(store.0.load(Ordering::SeqCst), 0);
    first.abort();
    second.abort();
    let _ = first.await;
    let _ = second.await;
    tokio::time::advance(std::time::Duration::from_secs(299)).await;
    settle().await;
    assert_eq!(store.0.load(Ordering::SeqCst), 0);
    tokio::time::advance(std::time::Duration::from_secs(1)).await;
    settle().await;
    assert_eq!(store.0.load(Ordering::SeqCst), 1);
    stop.send(true).unwrap();
    task.await.unwrap();
}
