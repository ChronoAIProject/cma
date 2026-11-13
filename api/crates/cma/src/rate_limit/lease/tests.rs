use super::*;
use std::{
    collections::VecDeque,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};

#[derive(Default)]
struct Store {
    calls: Mutex<Vec<u32>>,
    outcomes: Mutex<VecDeque<Result<Option<u64>, StoreError>>>,
    unavailable: AtomicBool,
    active: AtomicUsize,
    max_active: AtomicUsize,
}

#[async_trait::async_trait]
impl Charge for Store {
    async fn charge(&self, _: &str, _: u32, tokens: u32) -> Result<Option<u64>, StoreError> {
        self.calls.lock().unwrap().push(tokens);
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_active.fetch_max(active, Ordering::SeqCst);
        tokio::task::yield_now().await;
        self.active.fetch_sub(1, Ordering::SeqCst);
        if self.unavailable.load(Ordering::SeqCst) {
            return Err(StoreError::Unavailable("fixture offline".into()));
        }
        self.outcomes
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Ok(None))
    }
}

async fn demand(table: &LeaseTable, key: &str, arrivals: u32) {
    let entry = table.entry(key).unwrap();
    let mut lease = entry.lock().await;
    lease.previous_arrivals = arrivals;
}

#[tokio::test(start_paused = true)]
async fn observed_demand_grows_and_shrinks_and_expired_credits_are_discarded() {
    let table = LeaseTable::default();
    let store = Store::default();
    for _ in 0..12 {
        table.check(&store, "caller", 600).await.unwrap();
    }
    assert_eq!(*store.calls.lock().unwrap(), vec![1; 12]);
    tokio::time::advance(LEASE_DURATION).await;
    table.check(&store, "caller", 600).await.unwrap();
    assert_eq!(table.entry("caller").unwrap().lock().await.remaining, 11);
    assert_eq!(store.calls.lock().unwrap().last(), Some(&12));
    tokio::time::advance(LEASE_DURATION).await;
    table.check(&store, "caller", 600).await.unwrap();
    assert_eq!(table.entry("caller").unwrap().lock().await.remaining, 0);
    assert_eq!(store.calls.lock().unwrap().last(), Some(&1));
}

#[tokio::test(start_paused = true)]
async fn sparse_callers_stay_exact_and_batches_are_capped() {
    let table = LeaseTable::default();
    let store = Store::default();
    for _ in 0..3 {
        table.check(&store, "sparse", 600).await.unwrap();
        tokio::time::advance(LEASE_DURATION * 3).await;
    }
    assert_eq!(*store.calls.lock().unwrap(), [1, 1, 1]);
    demand(&table, "busy", u32::MAX).await;
    table.check(&store, "busy", 600).await.unwrap();
    assert_eq!(store.calls.lock().unwrap().last(), Some(&30));
    demand(&table, "tiny", u32::MAX).await;
    table.check(&store, "tiny", 1).await.unwrap();
    assert_eq!(store.calls.lock().unwrap().last(), Some(&1));
}

#[tokio::test(start_paused = true)]
async fn full_table_uses_exact_charges_and_reclaims_only_idle_expired_entries() {
    let table = LeaseTable::new(1);
    let store = Store::default();
    table.check(&store, "retained", 600).await.unwrap();
    let held = table.entry("retained").unwrap();
    for _ in 0..3 {
        table.check(&store, "overflow", 600).await.unwrap();
    }
    assert_eq!(table.table.lock().unwrap().entries.len(), 1);
    assert_eq!(*store.calls.lock().unwrap(), [1, 1, 1, 1]);
    tokio::time::advance(LEASE_DURATION).await;
    assert!(
        table.entry("overflow").is_none(),
        "a waiting caller pins the entry"
    );
    drop(held);
    tokio::time::advance(Duration::from_secs(1)).await;
    assert!(table.entry("overflow").is_some());
    assert!(!table.table.lock().unwrap().entries.contains_key("retained"));
}

#[tokio::test]
async fn concurrent_callers_share_one_prepaid_charge() {
    let table = LeaseTable::default();
    let store = Store::default();
    demand(&table, "caller", 64).await;
    let attempts = (0..64).map(|_| table.check(&store, "caller", 1280));
    for result in futures_util::future::join_all(attempts).await {
        result.unwrap();
    }
    assert_eq!(*store.calls.lock().unwrap(), [64]);
    assert_eq!(store.max_active.load(Ordering::SeqCst), 1);
    assert_eq!(table.entry("caller").unwrap().lock().await.remaining, 0);
}

#[tokio::test]
async fn rejected_batch_uses_single_request_result_and_retry_after() {
    for single in [None, Some(2)] {
        let table = LeaseTable::default();
        let store = Store::default();
        demand(&table, "caller", 30).await;
        store
            .outcomes
            .lock()
            .unwrap()
            .extend([Ok(Some(7)), Ok(single)]);
        assert_eq!(
            table.check(&store, "caller", 600).await,
            single.map_or(Ok(()), |delay| Err(Refusal::Limited(delay)))
        );
        assert_eq!(*store.calls.lock().unwrap(), [30, 1]);
        assert_eq!(table.entry("caller").unwrap().lock().await.remaining, 0);
    }
}

#[tokio::test(start_paused = true)]
async fn outage_spends_only_prepaid_credits_and_never_refills_or_falls_back() {
    let table = LeaseTable::default();
    let store = Store::default();
    demand(&table, "caller", 3).await;
    table.check(&store, "caller", 600).await.unwrap();
    store.unavailable.store(true, Ordering::SeqCst);
    table.check(&store, "caller", 600).await.unwrap();
    table.check(&store, "caller", 600).await.unwrap();
    assert_eq!(
        table.check(&store, "caller", 600).await,
        Err(Refusal::Unavailable)
    );
    assert_eq!(*store.calls.lock().unwrap(), [3, 3]);
    assert_eq!(table.entry("caller").unwrap().lock().await.remaining, 0);
    store.unavailable.store(false, Ordering::SeqCst);
    table.check(&store, "caller", 600).await.unwrap();
    store.unavailable.store(true, Ordering::SeqCst);
    tokio::time::advance(LEASE_DURATION).await;
    assert_eq!(
        table.check(&store, "caller", 600).await,
        Err(Refusal::Unavailable)
    );
    assert_eq!(table.entry("caller").unwrap().lock().await.remaining, 0);
    assert_eq!(
        LeaseTable::default().check(&store, "caller", 600).await,
        Err(Refusal::Unavailable)
    );
}

#[tokio::test]
async fn cancelling_an_inflight_charge_does_not_create_local_credit() {
    struct Pending(tokio::sync::Notify);
    #[async_trait::async_trait]
    impl Charge for Pending {
        async fn charge(&self, _: &str, _: u32, _: u32) -> Result<Option<u64>, StoreError> {
            self.0.notify_one();
            std::future::pending().await
        }
    }
    let table = Arc::new(LeaseTable::default());
    let pending = Arc::new(Pending(tokio::sync::Notify::new()));
    demand(&table, "caller", 30).await;
    let task_table = table.clone();
    let task_store = pending.clone();
    let task =
        tokio::spawn(async move { task_table.check(task_store.as_ref(), "caller", 600).await });
    pending.0.notified().await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    let unavailable = Store::default();
    unavailable.unavailable.store(true, Ordering::SeqCst);
    assert_eq!(
        table.check(&unavailable, "caller", 600).await,
        Err(Refusal::Unavailable)
    );
}
