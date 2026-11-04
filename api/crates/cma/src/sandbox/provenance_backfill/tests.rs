use super::*;
use layer2_core::{
    sandbox::store::{Page, SandboxStoreError},
    AgentId,
};
use std::sync::Mutex;

#[derive(Default)]
struct Store {
    scans: Mutex<Vec<Option<String>>>,
    processed: Mutex<Vec<String>>,
    done: tokio::sync::Notify,
    block: bool,
}
#[async_trait::async_trait]
impl AgentProvenanceBackfillStore for Store {
    async fn scan_agent_provenance_backfill(
        &self,
        page: &PageRequest,
    ) -> Result<Page<AgentId>, SandboxStoreError> {
        assert_eq!(page.limit, PAGE_SIZE);
        self.scans.lock().unwrap().push(page.cursor.clone());
        Ok(if page.cursor.is_none() {
            Page {
                items: vec![AgentId("a".into()), AgentId("b".into())],
                next: Some("b".into()),
            }
        } else {
            Page {
                items: vec![AgentId("c".into())],
                next: None,
            }
        })
    }
    async fn backfill_agent_provenance(
        &self,
        id: &AgentId,
    ) -> Result<ProvenanceBackfillOutcome, SandboxStoreError> {
        self.processed.lock().unwrap().push(id.to_string());
        if self.block {
            self.done.notify_one();
            std::future::pending::<()>().await;
        }
        if id.as_str() == "c" {
            self.done.notify_one();
        }
        if id.as_str() == "b" {
            return Err(SandboxStoreError::Corrupt("fixture".into()));
        }
        Ok(ProvenanceBackfillOutcome::Recorded)
    }
}

#[tokio::test(start_paused = true)]
async fn backfill_defers_startup_throttles_and_advances_past_corrupt_rows() {
    let store = Arc::new(Store::default());
    let (sender, receiver) = watch::channel(false);
    let mut backfill = AgentProvenanceBackfill::new(store.clone());
    backfill.budget = crate::background::BackgroundBudget::new();
    let worker = backfill.spawn(receiver);
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(29)).await;
    assert!(store.scans.lock().unwrap().is_empty());
    store.done.notified().await;
    assert_eq!(*store.processed.lock().unwrap(), ["a", "b", "c"]);
    assert_eq!(*store.scans.lock().unwrap(), [None, Some("b".into())]);
    sender.send(true).unwrap();
    worker.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn backfill_shutdown_cancels_pending_io_without_starting_another_row() {
    let store = Arc::new(Store {
        block: true,
        ..Store::default()
    });
    let (sender, receiver) = watch::channel(false);
    let mut backfill = AgentProvenanceBackfill::new(store.clone());
    backfill.budget = crate::background::BackgroundBudget::new();
    let worker = backfill.spawn(receiver);
    store.done.notified().await;
    sender.send(true).unwrap();
    worker.await.unwrap();
    assert_eq!(*store.processed.lock().unwrap(), ["a"]);
    assert_eq!(store.scans.lock().unwrap().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_busy_shared_budget_defers_backfill_without_advancing_its_cursor() {
    let budget = crate::background::BackgroundBudget::new();
    let release = Arc::new(tokio::sync::Notify::new());
    let mut holders = Vec::new();
    for name in ["agent_reconciliation", "sandbox_reconciliation"] {
        let work = crate::background::BackgroundWork::new(
            budget.clone(),
            name,
            Duration::from_secs(60),
            Duration::ZERO,
        );
        let release = release.clone();
        holders.push(tokio::spawn(async move {
            work.run(|| async {
                release.notified().await;
                Ok(())
            })
            .await
        }));
    }
    tokio::task::yield_now().await;
    let store = Arc::new(Store::default());
    let (sender, receiver) = watch::channel(false);
    let mut backfill = AgentProvenanceBackfill::new(store.clone());
    backfill.budget = budget;
    let worker = backfill.spawn(receiver);
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(31)).await;
    tokio::task::yield_now().await;
    assert!(store.scans.lock().unwrap().is_empty());
    release.notify_waiters();
    for holder in holders {
        holder.await.unwrap().unwrap();
    }
    store.done.notified().await;
    assert_eq!(*store.processed.lock().unwrap(), ["a", "b", "c"]);
    assert_eq!(*store.scans.lock().unwrap(), [None, Some("b".into())]);
    sender.send(true).unwrap();
    worker.await.unwrap();
}
