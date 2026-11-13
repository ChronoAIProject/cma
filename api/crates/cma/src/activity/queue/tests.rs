use super::*;
use layer2_core::{ActivityAction, ActivityOutcome, ActivityQuery, Page, StoreError};
use std::{
    sync::{atomic::AtomicUsize, Mutex},
    time::Duration,
};
use tokio::sync::watch;

#[derive(Default)]
struct Store {
    records: Mutex<Vec<ActivityRecord>>,
    batches: Mutex<Vec<Vec<ActivityRecord>>>,
    failures: AtomicUsize,
    blocked: AtomicBool,
    release: tokio::sync::Notify,
}

#[async_trait::async_trait]
impl ActivityStore for Store {
    async fn append(&self, _: &ActivityRecord) -> Result<(), StoreError> {
        panic!("queue must use batch port")
    }
    async fn append_many(&self, records: &[ActivityRecord]) -> Result<(), StoreError> {
        self.batches.lock().unwrap().push(records.to_vec());
        if self.blocked.load(Ordering::Relaxed) {
            self.release.notified().await;
        }
        if self
            .failures
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_sub(1))
            .is_ok()
        {
            return Err(StoreError::Unavailable("injected".into()));
        }
        self.records.lock().unwrap().extend_from_slice(records);
        Ok(())
    }
    async fn list(&self, _: &ActivityQuery) -> Result<Page<ActivityRecord>, StoreError> {
        Ok(Page::new(self.records.lock().unwrap().clone(), None))
    }
}

fn record() -> ActivityRecord {
    ActivityRecord::new(ActivityAction::Read, ActivityOutcome::Succeeded)
}

async fn advance(millis: u64) {
    tokio::time::advance(Duration::from_millis(millis)).await;
    tokio::task::yield_now().await;
}

#[tokio::test(start_paused = true)]
async fn batch_preserves_records_and_flushes_at_256_or_250_ms() {
    let store = Arc::new(Store::default());
    let (queue, worker) = ActivityQueue::new(store.clone());
    let (stop, signal) = watch::channel(false);
    let worker = worker.spawn(signal);
    let records: Vec<_> = (0..257).map(|_| record()).collect();
    for record in &records {
        assert!(queue.try_enqueue(record.clone()));
    }
    wait_batches(&store, 1).await;
    assert_eq!(store.records.lock().unwrap().len(), 256);
    advance(249).await;
    assert_eq!(store.records.lock().unwrap().len(), 256);
    advance(1).await;
    assert_eq!(*store.records.lock().unwrap(), records);
    assert_eq!(queue.shared.records.load(Ordering::Acquire), 0);
    assert_eq!(queue.shared.bytes.load(Ordering::Acquire), 0);
    stop.send(true).unwrap();
    worker.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn retries_keep_the_same_batch_and_reservations_with_capped_backoff() {
    let store = Arc::new(Store::default());
    store.failures.store(8, Ordering::Relaxed);
    let (queue, worker) = ActivityQueue::bounded(store.clone(), 1, MAX_BYTES);
    let (stop, signal) = watch::channel(false);
    let worker = worker.spawn(signal);
    let entry = record();
    assert!(queue.try_enqueue(entry.clone()));
    advance(250).await;
    let bytes = queue.shared.bytes.load(Ordering::Acquire);
    for (attempt, delay) in [100, 200, 400, 800, 1600, 3200, 5000, 5000]
        .into_iter()
        .enumerate()
    {
        assert_eq!(store.batches.lock().unwrap().len(), attempt + 1);
        assert_eq!(queue.shared.records.load(Ordering::Acquire), 1);
        assert_eq!(queue.shared.bytes.load(Ordering::Acquire), bytes);
        assert!(!queue.try_enqueue(record()));
        advance(delay - 1).await;
        assert_eq!(store.batches.lock().unwrap().len(), attempt + 1);
        advance(1).await;
    }
    assert_eq!(*store.records.lock().unwrap(), [entry]);
    assert!(store
        .batches
        .lock()
        .unwrap()
        .windows(2)
        .all(|batches| batches[0] == batches[1]));
    assert_eq!(queue.shared.records.load(Ordering::Acquire), 0);
    stop.send(true).unwrap();
    worker.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn overflow_counts_new_records_and_budgets_include_stalled_batch() {
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    let _recording = ::metrics::set_default_local_recorder(&recorder);
    let store = Arc::new(Store::default());
    store.blocked.store(true, Ordering::Relaxed);
    let (queue, worker) = ActivityQueue::new(store.clone());
    let (stop, signal) = watch::channel(false);
    let worker = worker.spawn(signal);
    for _ in 0..MAX_RECORDS {
        assert!(queue.try_enqueue(record()));
    }
    wait_batches(&store, 1).await;
    assert_eq!(store.batches.lock().unwrap().len(), 1);
    assert!(!queue.try_enqueue(record()));
    assert_eq!(queue.shared.records.load(Ordering::Acquire), MAX_RECORDS);
    assert!(queue.shared.bytes.load(Ordering::Acquire) <= MAX_BYTES);
    advance(30_000).await;
    let rendered = handle.render();
    assert!(
        rendered.contains("cma_activity_queue_records 10000"),
        "{rendered}"
    );
    assert!(
        rendered.contains("cma_activity_dropped_total{reason=\"queue_full\"} 1"),
        "{rendered}"
    );
    assert!(
        rendered.contains("cma_activity_queue_oldest_seconds 30"),
        "{rendered}"
    );
    stop.send(true).unwrap();
    worker.abort();
    assert!(worker.await.unwrap_err().is_cancelled());
    assert_eq!(queue.shared.records.load(Ordering::Acquire), 0);
    assert_eq!(queue.shared.bytes.load(Ordering::Acquire), 0);
    assert!(!queue.try_enqueue(record()));
    let rendered = handle.render();
    assert!(
        rendered.contains("cma_activity_dropped_total{reason=\"shutdown\"} 10001"),
        "{rendered}"
    );
}

#[tokio::test]
async fn byte_budget_admits_exact_size_and_returns_capacity_on_worker_drop() {
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    let _recording = ::metrics::set_default_local_recorder(&recorder);
    let record = record();
    let size = serde_json::to_vec(&record).unwrap().len();
    let (queue, worker) = ActivityQueue::bounded(Arc::new(Store::default()), 10, size);
    assert!(queue.try_enqueue(record.clone()));
    assert!(!queue.try_enqueue(record.clone()));
    assert_eq!(queue.shared.bytes.load(Ordering::Acquire), size);
    assert_eq!(queue.shared.records.load(Ordering::Acquire), 1);
    assert!(handle
        .render()
        .contains("cma_activity_dropped_total{reason=\"byte_budget\"} 1"));
    drop(worker);
    assert_eq!(queue.shared.bytes.load(Ordering::Acquire), 0);
    assert!(!queue.try_enqueue(record));
}

#[tokio::test(start_paused = true)]
async fn coordinator_drains_partial_batches_without_waiting_for_batch_interval() {
    let store = Arc::new(Store::default());
    let (queue, worker) = ActivityQueue::new(store.clone());
    let records: Vec<_> = (0..513).map(|_| record()).collect();
    for record in &records {
        assert!(queue.try_enqueue(record.clone()));
    }
    let mut shutdown = crate::shutdown::ShutdownCoordinator::new();
    shutdown.push(worker.spawn(shutdown.subscribe()));
    let start = Instant::now();
    assert_eq!(shutdown.drain().await, 0);
    assert_eq!(start.elapsed(), Duration::ZERO);
    assert_eq!(*store.records.lock().unwrap(), records);
    assert!(!queue.try_enqueue(record()));
}

#[tokio::test(start_paused = true)]
async fn coordinator_deadline_counts_all_outstanding_and_closes_admission() {
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    let _recording = ::metrics::set_default_local_recorder(&recorder);
    let store = Arc::new(Store::default());
    store.blocked.store(true, Ordering::Relaxed);
    let (queue, worker) = ActivityQueue::new(store);
    for _ in 0..300 {
        assert!(queue.try_enqueue(record()));
    }
    let mut shutdown = crate::shutdown::ShutdownCoordinator::new();
    shutdown.push(worker.spawn(shutdown.subscribe()));
    let start = Instant::now();
    assert_eq!(shutdown.drain().await, 1);
    assert_eq!(start.elapsed(), Duration::from_secs(30));
    assert_eq!(queue.shared.records.load(Ordering::Acquire), 0);
    assert_eq!(queue.shared.bytes.load(Ordering::Acquire), 0);
    assert!(handle
        .render()
        .contains("cma_activity_dropped_total{reason=\"shutdown\"} 300"));
    assert!(!queue.try_enqueue(record()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_producers_cannot_exceed_either_budget() {
    let record = record();
    let size = serde_json::to_vec(&record).unwrap().len();
    let (queue, worker) = ActivityQueue::bounded(Arc::new(Store::default()), 1000, size * 500);
    let mut producers = Vec::new();
    for _ in 0..8 {
        let queue = queue.clone();
        let record = record.clone();
        producers.push(tokio::spawn(async move {
            (0..200)
                .filter(|_| queue.try_enqueue(record.clone()))
                .count()
        }));
    }
    let mut accepted = 0;
    for producer in producers {
        accepted += producer.await.unwrap();
    }
    assert_eq!(accepted, 500);
    assert_eq!(queue.shared.records.load(Ordering::Acquire), 500);
    assert_eq!(queue.shared.bytes.load(Ordering::Acquire), size * 500);
    drop(worker);
    assert_eq!(queue.shared.records.load(Ordering::Acquire), 0);
}

async fn wait_batches(store: &Store, count: usize) {
    for _ in 0..100 {
        if store.batches.lock().unwrap().len() >= count {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("worker did not begin expected batch");
}

#[tokio::test]
async fn a_record_exceeding_sixteen_mib_is_rejected_without_consuming_count_budget() {
    let (queue, _worker) = ActivityQueue::new(Arc::new(Store::default()));
    let huge = record().on(layer2_core::ActivityTarget::Resource("x".repeat(MAX_BYTES)));
    assert!(!queue.try_enqueue(huge));
    assert_eq!(queue.shared.records.load(Ordering::Acquire), 0);
    assert_eq!(queue.shared.bytes.load(Ordering::Acquire), 0);
    assert!(queue.try_enqueue(record()));
}

#[tokio::test(start_paused = true)]
async fn overdue_backlog_still_flushes_in_full_batches_after_retry() {
    let store = Arc::new(Store::default());
    store.failures.store(3, Ordering::Relaxed);
    let (queue, worker) = ActivityQueue::new(store.clone());
    let (stop, signal) = watch::channel(false);
    let worker = worker.spawn(signal);
    for _ in 0..768 {
        assert!(queue.try_enqueue(record()));
    }
    wait_batches(&store, 1).await;
    for delay in [100, 200, 400] {
        advance(delay).await;
    }
    wait_batches(&store, 6).await;
    assert_eq!(store.records.lock().unwrap().len(), 768);
    assert!(store
        .batches
        .lock()
        .unwrap()
        .iter()
        .all(|batch| batch.len() == 256));
    stop.send(true).unwrap();
    worker.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn shutdown_preserves_a_write_in_progress_and_drains_queued_tail() {
    let store = Arc::new(Store::default());
    store.blocked.store(true, Ordering::Relaxed);
    let (queue, worker) = ActivityQueue::new(store.clone());
    let mut shutdown = crate::shutdown::ShutdownCoordinator::new();
    shutdown.push(worker.spawn(shutdown.subscribe()));
    let records: Vec<_> = (0..300).map(|_| record()).collect();
    for record in &records {
        assert!(queue.try_enqueue(record.clone()));
    }
    wait_batches(&store, 1).await;
    shutdown.signal();
    for _ in 0..100 {
        if !queue.shared.accepting.load(Ordering::Acquire) {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert!(!queue.try_enqueue(record()));
    store.blocked.store(false, Ordering::Relaxed);
    store.release.notify_one();
    assert_eq!(shutdown.drain().await, 0);
    assert_eq!(*store.records.lock().unwrap(), records);
    assert_eq!(store.batches.lock().unwrap().len(), 2);
    assert_eq!(queue.shared.records.load(Ordering::Acquire), 0);
}
