use super::*;
use std::sync::{atomic::Ordering, Arc};
use std::time::Duration;

#[path = "maintenance_test_support.rs"]
mod support;
use support::*;

#[tokio::test(start_paused = true)]
async fn reclaim_deletes_only_after_exclusive_claim() {
    for already_condemned in [false, true] {
        let (store, blobs, first) = fixture();
        store.reserve(source(1), 0).await;
        if already_condemned {
            store.set_state(&source(1), State::Condemned).await;
        }
        blobs.pause();
        let second = DocumentMaintenance::new(store.clone(), blobs.clone(), true);
        let a = tokio::spawn(async move { first.reclaim_step(NOW).await });
        blobs.wait_for_calls(1).await;
        let b = tokio::spawn(async move { second.reclaim_step(NOW).await });
        for _ in 0..20 {
            tokio::task::yield_now().await;
        }
        let calls = blobs.calls.load(Ordering::SeqCst);
        blobs.resume(2);
        let a = a.await.unwrap().unwrap();
        let b = b.await.unwrap().unwrap();
        assert_eq!(calls, 1, "only one worker may delete a reservation");
        assert_eq!(a.released + b.released, 1);
        assert_eq!(store.counters().await, [0, 0, 0]);
    }
}

#[tokio::test]
async fn document_reclaim_condemned_row_without_claim_is_never_deleted() {
    for already_condemned in [false, true] {
        let (store, blobs, maintenance) = fixture();
        store.reserve(source(1), 0).await;
        if already_condemned {
            store.set_state(&source(1), State::Condemned).await;
        }
        store.refuse_claim.store(true, Ordering::SeqCst);
        let outcome = maintenance.reclaim_step(NOW).await.unwrap();
        assert_eq!(blobs.calls.load(Ordering::SeqCst), 0);
        assert_eq!((outcome.deleted, outcome.released), (0, 0));
        assert_eq!(store.counters().await, [1, 8, 1]);
    }
}

#[tokio::test(start_paused = true)]
async fn reclaim_skips_delete_when_lease_cannot_cover_operation_budget() {
    for (delay_ms, expected_deletes) in [(264_000, 1), (265_000, 0), (266_000, 0), (300_001, 0)] {
        let (store, blobs, maintenance) = fixture();
        store.reserve(source(1), 0).await;
        store.claim_delay_ms.store(delay_ms, Ordering::SeqCst);
        let outcome = maintenance.reclaim_step(NOW).await.unwrap();
        assert_eq!(blobs.calls.load(Ordering::SeqCst), expected_deletes);
        assert_eq!(outcome.released, expected_deletes as u64);
        if expected_deletes == 0 {
            assert_eq!(store.state(&source(1)).await, Some(State::Condemned));
            assert_eq!(store.counters().await, [1, 8, 1]);
        }
    }
}

#[tokio::test]
async fn reclaim_blocks_without_ready_ledger_or_supported_deletion() {
    for (enabled, ready, supported, reason) in [
        (false, true, true, "reclaim_disabled"),
        (true, false, true, "ledger_not_ready"),
        (true, true, false, "storage_deletion_unsupported"),
    ] {
        let store = Arc::new(Store::new(ready));
        store.reserve(source(1), 0).await;
        let blobs = Arc::new(Blobs::new(supported));
        let maintenance = DocumentMaintenance::new(store.clone(), blobs.clone(), enabled);
        let outcome = maintenance.reclaim_step(NOW).await.unwrap();
        assert!(outcome.blocked);
        assert_eq!(outcome.blocked_reason, Some(reason));
        assert_eq!(store.reads.load(Ordering::SeqCst), 0);
        assert_eq!(store.judgements.load(Ordering::SeqCst), 0);
        assert_eq!(blobs.calls.load(Ordering::SeqCst), 0);
        assert_eq!(store.state(&source(1)).await, Some(State::Reserved));
    }
}

#[tokio::test]
async fn two_tab_conflict_orphan_is_condemned_deleted_and_released() {
    let (store, blobs, maintenance) = fixture();
    let committed = source(1);
    let orphan = source(2);
    store.reserve(committed.clone(), 0).await;
    store.reserve(orphan.clone(), 0).await;
    store.reference(&committed).await;
    blobs.put(&committed).await;
    blobs.put(&orphan).await;
    let outcome = maintenance.reclaim_step(NOW).await.unwrap();
    assert_eq!(
        (outcome.judged, outcome.committed, outcome.condemned),
        (2, 1, 1)
    );
    assert_eq!((outcome.deleted, outcome.released), (1, 1));
    assert_eq!(store.state(&committed).await, Some(State::Committed));
    assert_eq!(store.state(&orphan).await, None);
    assert!(blobs.contains(&committed).await);
    assert!(!blobs.contains(&orphan).await);
    assert_eq!(store.counters().await, [1, committed.bytes as i64, 1]);
}

#[tokio::test]
async fn failed_create_releases_document_slot_after_last_object() {
    let (store, blobs, maintenance) = fixture();
    store.reserve(source(1), 0).await;
    store.reserve(source(2), 0).await;
    blobs.put(&source(1)).await;
    let outcome = maintenance.reclaim_step(NOW).await.unwrap();
    // The second delete observes absence, which also proves finality.
    assert_eq!((outcome.deleted, outcome.released), (2, 2));
    assert_eq!(store.counters().await, [0, 0, 0]);
    assert_eq!(outcome.backlog.pending_objects, 0);
    assert_eq!(maintenance.reclaim_step(NOW).await.unwrap().released, 0);
}

#[tokio::test]
async fn referenced_baseline_row_is_marked_committed_not_condemned() {
    let (store, blobs, maintenance) = fixture();
    let source = source(1);
    store.reserve(source.clone(), 0).await;
    store.set_state(&source, State::Baseline).await;
    store.reference(&source).await;
    blobs.put(&source).await;
    let outcome = maintenance.reclaim_step(NOW).await.unwrap();
    assert_eq!(outcome.committed, 1);
    assert_eq!(outcome.condemned, 0);
    assert_eq!(blobs.calls.load(Ordering::SeqCst), 0);
    assert_eq!(store.state(&source).await, Some(State::Committed));
}

#[tokio::test(start_paused = true)]
async fn delete_failure_keeps_condemned_and_reports_retry_after() {
    let (store, blobs, maintenance) = fixture();
    let source = source(1);
    store.reserve(source.clone(), 0).await;
    blobs.put(&source).await;
    blobs
        .fail(BlobMaintenanceError {
            error: DocumentError::Unavailable,
            retry_after: Some(Duration::from_secs(900)),
        })
        .await;
    let outcome = maintenance.reclaim_step(NOW).await.unwrap();
    assert_eq!(outcome.delete_failed, 1);
    assert_eq!(outcome.released, 0);
    assert_eq!(outcome.retry_after, Some(Duration::from_secs(900)));
    assert_eq!(store.state(&source).await, Some(State::Condemned));
    assert_eq!(outcome.backlog.pending_objects, 1);
    assert!(blobs.contains(&source).await);
    assert_eq!(store.counters().await, [1, source.bytes as i64, 1]);
    // A failed delete retains its exclusive lease until it strictly expires.
    assert_eq!(maintenance.reclaim_step(NOW).await.unwrap().released, 0);
    let retried = maintenance
        .reclaim_step(NOW + DELETE_LEASE.as_millis() as i64 + 1)
        .await
        .unwrap();
    assert_eq!((retried.judged, retried.released), (0, 1));
}

#[tokio::test]
async fn release_never_makes_counters_negative() {
    let (store, _, maintenance) = fixture();
    let source = source(1);
    store.reserve(source.clone(), 0).await;
    store.corrupt_counters([1, 0, 0]).await;
    assert_eq!(
        maintenance.reclaim_step(NOW).await,
        Err(DocumentError::Corrupt)
    );
    assert_eq!(store.counters().await, [1, 0, 0]);
    assert_eq!(store.state(&source).await, Some(State::Condemned));
}

#[tokio::test]
async fn reclaim_delete_concurrency_is_bounded_and_drains_existing_work_after_failure() {
    let (store, blobs, maintenance) = fixture();
    for index in 0..10 {
        store.reserve(source(index), 0).await;
    }
    blobs.pause();
    blobs
        .fail(BlobMaintenanceError {
            error: DocumentError::Unavailable,
            retry_after: None,
        })
        .await;
    let running = tokio::spawn(async move { maintenance.reclaim_step(NOW).await });
    tokio::time::timeout(
        Duration::from_secs(2),
        blobs.wait_for_calls(RECLAIM_DELETE_CONCURRENCY),
    )
    .await
    .expect("the initial delete batch starts");
    assert_eq!(
        blobs.calls.load(Ordering::SeqCst),
        RECLAIM_DELETE_CONCURRENCY
    );
    blobs.resume(RECLAIM_BATCH);
    let outcome = running.await.unwrap().unwrap();
    assert_eq!(
        blobs.calls.load(Ordering::SeqCst),
        RECLAIM_DELETE_CONCURRENCY
    );
    assert_eq!(
        blobs.maximum.load(Ordering::SeqCst),
        RECLAIM_DELETE_CONCURRENCY
    );
    assert_eq!(outcome.delete_failed, 1);
    assert_eq!(outcome.released, (RECLAIM_DELETE_CONCURRENCY - 1) as u64);
    assert_eq!(outcome.retry_after, Some(RECLAIM_INTERVAL));
}

#[tokio::test]
async fn reclaim_honours_strict_grace_and_batch_with_condemned_rows_first() {
    let (store, _, maintenance) = fixture();
    let cutoff = NOW - RECLAIM_GRACE.as_millis() as i64;
    store.reserve(source(0), cutoff).await;
    for index in 1..=RECLAIM_BATCH + 2 {
        store.reserve(source(index), 0).await;
    }
    store
        .set_state(&source(RECLAIM_BATCH + 2), State::Condemned)
        .await;
    let candidates = store.candidates(cutoff, RECLAIM_BATCH).await.unwrap();
    assert!(candidates[0].condemned);
    assert_eq!(candidates.len(), RECLAIM_BATCH);
    let outcome = maintenance.reclaim_step(NOW).await.unwrap();
    assert_eq!(outcome.released, RECLAIM_BATCH as u64);
    assert_eq!(store.state(&source(0)).await, Some(State::Reserved));
    assert_eq!(outcome.backlog.pending_objects, 2);
}
