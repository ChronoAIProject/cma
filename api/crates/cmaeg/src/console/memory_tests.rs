use std::{
    sync::{atomic::Ordering, Arc},
    time::{Duration, Instant},
};

use super::*;
use crate::{
    channels::observer::ChannelIndexObserver,
    console::{
        observer::ConsoleObserver,
        test_support::{authorized, record, FakeHeads, FakeReader},
    },
    records::RecordHead,
};

fn sources(heads: Arc<FakeHeads>) -> Sources {
    Sources {
        heads,
        reader: Arc::new(FakeReader::default()),
    }
}

fn config(max_records: usize) -> ConsoleMemoryConfig {
    ConsoleMemoryConfig {
        index_max_records: max_records,
        ..ConsoleMemoryConfig::default()
    }
}

fn schedule(settle: Duration, resync: Duration) -> Schedule {
    Schedule {
        settle,
        resync,
        pacing: Pacing::default(),
    }
}

async fn eventually(mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done() {
        assert!(Instant::now() < deadline, "condition not reached");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn listing_starts_only_after_readiness_and_its_settle() {
    let heads = Arc::new(FakeHeads::seeded(1_000));
    let memory = Arc::new(ConsoleMemory::new(
        &config(100_000),
        sources(heads.clone()),
        schedule(Duration::from_millis(200), Duration::from_secs(3_600)),
    ));
    let (_observer, queue) = ConsoleObserver::new();
    let readiness = Readiness::initialized();
    memory.spawn(queue, readiness.clone());
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        heads.calls.lock().unwrap().is_empty(),
        "listed before readiness"
    );
    let ready_at = Instant::now();
    readiness.note_served_ready();
    eventually(|| {
        let status = memory.index_status();
        status.records == 1_000 && status.loading.is_none()
    })
    .await;
    let calls = heads.calls.lock().unwrap();
    assert_eq!(calls.len(), 11, "ten full pages and one empty");
    assert!(calls[0].1.duration_since(ready_at) >= Duration::from_millis(200));
    for pair in calls.windows(2) {
        assert!(pair[1].1.duration_since(pair[0].2) >= Duration::from_millis(50));
    }
    assert_eq!(heads.max_inflight.load(Ordering::SeqCst), 1);
    let status = memory.index_status();
    assert_eq!(status.loading, None);
    assert!(!status.partial);
    let alice = authorized("alice");
    assert_eq!(memory.index.records_for(&alice).len(), 500);
}

#[tokio::test]
async fn observations_keep_the_index_current() {
    let memory = Arc::new(ConsoleMemory::new(
        &config(1_000),
        sources(Arc::new(FakeHeads::default())),
        schedule(Duration::from_secs(3_600), Duration::from_secs(3_600)),
    ));
    let (observer, queue) = ConsoleObserver::new();
    memory.spawn(queue, Readiness::initialized());
    observer.changed(&record(1), "alice", 1);
    observer.changed(&record(2), "alice", 1);
    observer.changed(&record(1), "alice", 2);
    observer.removed(&record(2));
    let alice = authorized("alice");
    eventually(|| memory.index.records_for(&alice) == [(record(1), 2)]).await;
}

#[tokio::test]
async fn an_owner_is_reconciled_at_most_once_per_interval() {
    let heads = Arc::new(FakeHeads::seeded(4));
    let memory = ConsoleMemory::new(
        &config(1_000),
        sources(heads.clone()),
        schedule(Duration::from_secs(3_600), Duration::from_secs(3_600)),
    );
    // A stale head the owner's listing no longer has is retired.
    memory.index.upsert(record(90), "alice".into(), 1);
    let alice = authorized("alice");
    assert!(memory.reconcile(&alice).await);
    assert!(!memory.reconcile(&alice).await);
    assert_eq!(
        memory.index.records_for(&alice),
        [(record(0), 1), (record(2), 1)]
    );
    // Another owner has its own interval.
    assert!(memory.reconcile(&authorized("bob")).await);
    let calls = heads.calls.lock().unwrap();
    let owners: Vec<_> = calls.iter().map(|call| call.0.clone()).collect();
    assert_eq!(owners, [Some("alice".to_owned()), Some("bob".to_owned())]);
}

#[tokio::test]
async fn a_failed_reconcile_may_be_retried_at_once() {
    let heads = Arc::new(FakeHeads::seeded(4));
    *heads.fail.lock().unwrap() = true;
    let memory = ConsoleMemory::new(
        &config(1_000),
        sources(heads.clone()),
        schedule(Duration::from_secs(3_600), Duration::from_secs(3_600)),
    );
    memory.index.upsert(record(0), "alice".into(), 1);
    let alice = authorized("alice");
    assert!(memory.reconcile(&alice).await);
    // Nothing is removed on a failed listing.
    assert_eq!(memory.index.records_for(&alice), [(record(0), 1)]);
    *heads.fail.lock().unwrap() = false;
    assert!(memory.reconcile(&alice).await);
}

#[tokio::test]
async fn a_resync_drops_records_cma_no_longer_has() {
    let heads = Arc::new(FakeHeads::seeded(3));
    let memory = Arc::new(ConsoleMemory::new(
        &config(1_000),
        sources(heads.clone()),
        schedule(Duration::ZERO, Duration::from_millis(100)),
    ));
    let (_observer, queue) = ConsoleObserver::new();
    let readiness = Readiness::initialized();
    readiness.note_served_ready();
    memory.spawn(queue, readiness);
    eventually(|| memory.index_status().records == 3).await;
    heads
        .heads
        .lock()
        .unwrap()
        .retain(|head: &RecordHead| head.record_id != record(1).to_string());
    eventually(|| memory.index_status().records == 2).await;
    assert_eq!(
        memory.index.version_for(&authorized("bob"), &record(1)),
        None
    );
}

#[tokio::test]
async fn a_failed_listing_keeps_the_index() {
    let heads = Arc::new(FakeHeads::seeded(3));
    let memory = ConsoleMemory::new(
        &config(1_000),
        sources(heads.clone()),
        schedule(Duration::ZERO, Duration::from_secs(3_600)),
    );
    memory.list_all().await;
    assert_eq!(memory.index_status().records, 3);
    *heads.fail.lock().unwrap() = true;
    memory.list_all().await;
    let status = memory.index_status();
    assert_eq!(status.records, 3);
    assert_eq!(status.loading, None);
}

#[tokio::test]
async fn a_listing_beyond_the_bound_marks_the_index_partial() {
    let heads = Arc::new(FakeHeads::seeded(150));
    let memory = ConsoleMemory::new(
        &config(100),
        sources(heads),
        schedule(Duration::ZERO, Duration::from_secs(3_600)),
    );
    memory.list_all().await;
    let status = memory.index_status();
    assert_eq!(status.records, 100);
    assert!(status.partial);
}
