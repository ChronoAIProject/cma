use super::*;
use std::sync::Arc;

pub(super) async fn fixture() -> Arc<ReportingStore> {
    let store = ReportingStore::connect(
        &std::env::var("CHRONO_TEST_MONGODB_URI").unwrap(),
        &format!("allocation_buckets_{}", uuid::Uuid::new_v4().simple()),
        &[37; 32],
    )
    .await
    .unwrap();
    store.initialize_allocation_buckets(true).await.unwrap();
    store
}
pub(super) async fn ready(store: &ReportingStore, lane: Lane) {
    store
        .collection(CONTROL)
        .update_one(
            doc! {"_id":lane.key()},
            doc! {"$set":{
            "lease_until":mongodb::bson::DateTime::from_millis(0),
            "next_at":mongodb::bson::DateTime::from_millis(0)}},
        )
        .await
        .unwrap();
}
pub(super) async fn seed(store: &ReportingStore, states: &[SandboxUsage]) {
    store
        .collection(super::super::super::sandbox_usage::STATES)
        .insert_many(
            states
                .iter()
                .map(|s| super::super::super::sandbox_usage::state_document(s).unwrap()),
        )
        .await
        .unwrap();
}
pub(super) async fn checkpoint(store: &ReportingStore, lane: Lane) -> Document {
    store
        .collection(CONTROL)
        .find_one(doc! {"_id":lane.key()})
        .await
        .unwrap()
        .unwrap()
}
pub(super) fn progress(from: u64) -> worker::Progress {
    // Serialize through the actual durable contract, including unknown-field refusal.
    serde_json::from_value(serde_json::json!({"day":from,"cutoff":from+DAY,
        "after":null,"pass_complete":true}))
    .unwrap()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn disabled_maintenance_and_initialization_create_no_bucket_or_control_records() {
    let store = ReportingStore::connect(
        &std::env::var("CHRONO_TEST_MONGODB_URI").unwrap(),
        &format!("allocation_off_{}", uuid::Uuid::new_v4().simple()),
        &[37; 32],
    )
    .await
    .unwrap();
    let control = store
        .collection(CONTROL)
        .count_documents(doc! {})
        .await
        .unwrap();
    store.initialize_allocation_buckets(false).await.unwrap();
    for lane in [Lane::Live, Lane::Backfill] {
        assert!(
            !store
                .maintain_allocation_buckets(false, lane)
                .await
                .unwrap()
                .claimed
        );
    }
    assert!(!store
        .db
        .list_collection_names()
        .await
        .unwrap()
        .contains(&COLLECTION.into()));
    assert_eq!(
        store
            .collection(CONTROL)
            .count_documents(doc! {})
            .await
            .unwrap(),
        control
    );
    store.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn lease_expiry_takeover_and_release_fence_data_and_progress() {
    let store = fixture().await;
    let old = store
        .claim_allocation_buckets(Lane::Backfill)
        .await
        .unwrap()
        .unwrap();
    assert!(store
        .claim_allocation_buckets(Lane::Backfill)
        .await
        .unwrap()
        .is_none());
    let from = (old.at / DAY - 1) * DAY;
    let s = state(1, from);
    let bucket = materialize::build(
        &s,
        &[],
        Range {
            from,
            to: from + DAY,
        },
    )
    .unwrap();
    ready(&store, Lane::Backfill).await;
    assert!(store
        .publish_allocation_buckets(&old, std::slice::from_ref(&bucket), &progress(from))
        .await
        .is_err());
    let next = store
        .claim_allocation_buckets(Lane::Backfill)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(old.holder, next.holder);
    assert!(next.generation > old.generation);
    assert!(store
        .publish_allocation_buckets(&old, std::slice::from_ref(&bucket), &progress(from))
        .await
        .is_err());
    assert_eq!(
        store
            .collection(COLLECTION)
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    assert!(!checkpoint(&store, Lane::Backfill)
        .await
        .contains_key("record"));
    store
        .publish_allocation_buckets(&next, std::slice::from_ref(&bucket), &progress(from))
        .await
        .unwrap();
    assert!(store
        .publish_allocation_buckets(&next, &[], &progress(from))
        .await
        .is_err());
    let row = store
        .collection(COLLECTION)
        .find_one(doc! {})
        .await
        .unwrap()
        .unwrap();
    assert_projection(
        &DayBucket::read(&row).unwrap().total().projection,
        &bucket.total().projection,
    );
    store.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn publication_failure_rolls_back_earlier_bucket_and_checkpoint() {
    let store = fixture().await;
    let lease = store
        .claim_allocation_buckets(Lane::Backfill)
        .await
        .unwrap()
        .unwrap();
    let from = (lease.at / DAY - 1) * DAY;
    let buckets: Vec<_> = (1..=2)
        .map(|id| {
            materialize::build(
                &state(id, from),
                &[],
                Range {
                    from,
                    to: from + DAY,
                },
            )
            .unwrap()
        })
        .collect();
    let mut corrupt = buckets[1].document().unwrap();
    corrupt.insert("content_sha256", "corrupt");
    store
        .collection(COLLECTION)
        .insert_one(corrupt)
        .await
        .unwrap();
    assert!(store
        .publish_allocation_buckets(&lease, &buckets, &progress(from))
        .await
        .is_err());
    assert!(store
        .collection(COLLECTION)
        .find_one(doc! {"_id":buckets[0].key()})
        .await
        .unwrap()
        .is_none());
    assert!(!checkpoint(&store, Lane::Backfill)
        .await
        .contains_key("record"));
    assert_eq!(
        store
            .collection(COLLECTION)
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    store.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn multi_replica_backfill_resumes_pages_and_reuses_unchanged_history() {
    let store = fixture().await;
    let from = (super::super::super::usage_reports::now() / DAY - 1) * DAY;
    let page = worker::PAGE_SIZE as usize;
    let states: Vec<_> = (1..=page + 2).map(|id| state(id as u64, from)).collect();
    seed(&store, &states).await;
    let other = ReportingStore::connect(
        &std::env::var("CHRONO_TEST_MONGODB_URI").unwrap(),
        store.db.name(),
        &[37; 32],
    )
    .await
    .unwrap();
    other.initialize_allocation_buckets(true).await.unwrap();
    let (a, b) = tokio::join!(
        store.maintain_allocation_buckets(true, Lane::Backfill),
        other.maintain_allocation_buckets(true, Lane::Backfill)
    );
    let (a, b) = (a.unwrap(), b.unwrap());
    assert_eq!(a.written + b.written, page);
    assert_eq!(usize::from(a.claimed) + usize::from(b.claimed), 1);
    ready(&other, Lane::Backfill).await;
    let second = other
        .maintain_allocation_buckets(true, Lane::Backfill)
        .await
        .unwrap();
    assert_eq!(second.written, 2);
    assert_eq!(
        store
            .collection(COLLECTION)
            .count_documents(doc! {})
            .await
            .unwrap(),
        page as u64 + 2
    );
    // Reset only test progress to repeat yesterday; stable state avoids raw reads.
    store
        .collection(CONTROL)
        .update_one(
            doc! {"_id":Lane::Backfill.key()},
            doc! {"$unset":{"record":""}},
        )
        .await
        .unwrap();
    ready(&store, Lane::Backfill).await;
    let repeat = store
        .maintain_allocation_buckets(true, Lane::Backfill)
        .await
        .unwrap();
    assert_eq!(
        (repeat.reused, repeat.written, repeat.source_rows),
        (page, 0, 0)
    );
    store.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn maintained_30_day_history_matches_raw_and_new_heads_replace_projected_tails() {
    let original = fixture().await;
    let policy = crate::config::report_views::ViewPolicy {
        allocation_rollups_enabled: true,
        ..Default::default()
    };
    let store = original.with_view_policy(policy);
    let today = super::super::super::usage_reports::now() / DAY * DAY;
    let from = today - 30 * DAY;
    let mut s = state(1, from);
    s.last_observed_at = today;
    s.revision = 30;
    s.source_seq = 30;
    s.source_revision = 30;
    let events: Vec<_> = (0..30)
        .map(|n| event(&s, n + 1, from + n * DAY, from + (n + 1) * DAY))
        .collect();
    seed(&store, std::slice::from_ref(&s)).await;
    store
        .collection(super::super::super::sandbox_usage::EVENTS)
        .insert_many(
            events
                .iter()
                .map(|e| super::super::super::sandbox_usage::event_document(e).unwrap()),
        )
        .await
        .unwrap();
    for n in 0..30 {
        ready(&store, Lane::Backfill).await;
        let result = store
            .maintain_allocation_buckets(true, Lane::Backfill)
            .await
            .unwrap();
        assert_eq!((result.written, result.source_rows), (1, 1));
        assert_eq!(result.pass_complete, n == 29);
    }
    let mut session = store.snapshot().await.unwrap();
    let read = store
        .bucketed_allocation_range(
            &mut session,
            std::slice::from_ref(&s),
            Range { from, to: today },
        )
        .await
        .unwrap();
    assert_eq!(read.len(), 30);
    for (start, rows) in read {
        assert_projection(
            &rows[s.sandbox_id.as_str()],
            &allocation::project(
                &s,
                &events,
                Range {
                    from: start,
                    to: start + DAY,
                },
            ),
        );
    }
    // A source revision advance closes a previously projected tail exactly.
    let mut newer = state(2, today - DAY);
    seed(&store, std::slice::from_ref(&newer)).await;
    ready(&store, Lane::Backfill).await;
    let first = store
        .maintain_allocation_buckets(true, Lane::Backfill)
        .await
        .unwrap();
    assert_eq!((first.reused, first.written), (1, 1));
    let old = store
        .collection(COLLECTION)
        .find_one(doc! {"_id":format!("{}:{}",newer.sandbox_id,today-DAY)})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        DayBucket::read(&old)
            .unwrap()
            .total()
            .projection
            .period
            .projected_tail_seconds,
        DAY
    );
    newer.revision = 2;
    newer.last_observed_at = today;
    newer.source_seq = 2;
    newer.source_revision = 2;
    store
        .collection(super::super::super::sandbox_usage::STATES)
        .replace_one(
            doc! {"_id":newer.sandbox_id.as_str()},
            super::super::super::sandbox_usage::state_document(&newer).unwrap(),
        )
        .await
        .unwrap();
    store
        .collection(super::super::super::sandbox_usage::EVENTS)
        .insert_one(
            super::super::super::sandbox_usage::event_document(&event(
                &newer,
                2,
                today - DAY,
                today,
            ))
            .unwrap(),
        )
        .await
        .unwrap();
    store
        .collection(CONTROL)
        .update_one(
            doc! {"_id":Lane::Backfill.key()},
            doc! {"$unset":{"record":""}},
        )
        .await
        .unwrap();
    ready(&store, Lane::Backfill).await;
    let refresh = store
        .maintain_allocation_buckets(true, Lane::Backfill)
        .await
        .unwrap();
    assert_eq!(
        (refresh.reused, refresh.written, refresh.source_rows),
        (1, 1, 1)
    );
    let saved = store
        .collection(COLLECTION)
        .find_one(doc! {"_id":format!("{}:{}",newer.sandbox_id,today-DAY)})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        DayBucket::read(&saved)
            .unwrap()
            .total()
            .projection
            .period
            .projected_tail_seconds,
        0
    );
    store.db.drop().await.unwrap();
}
