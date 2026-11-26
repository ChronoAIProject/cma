use super::mongo::{fixture, ready, seed};
use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn live_boundary_refresh_tracks_source_heads_beyond_the_completed_hour() {
    let store = fixture().await;
    let now = super::super::super::usage_reports::now();
    let day = now / DAY * DAY;
    let observed = now.saturating_sub(90).max(day + 2);
    let middle = day + (observed - day) / 2;
    let mut s = state(1, day);
    s.revision = 2;
    s.source_seq = 2;
    s.source_revision = 2;
    s.last_observed_at = observed;
    let mut events = vec![event(&s, 1, day, middle), event(&s, 2, middle, observed)];
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
    let first = store
        .maintain_allocation_buckets(true, Lane::Live)
        .await
        .unwrap();
    assert_eq!((first.written, first.source_rows), (1, 2));
    let doc = store
        .collection(COLLECTION)
        .find_one(doc! {"sandbox_id":s.sandbox_id.as_str(),"day":day as i64})
        .await
        .unwrap()
        .unwrap();
    let old = DayBucket::read_boundary(&doc).unwrap();
    let range = Range {
        from: day + 1,
        to: observed + 45,
    };
    assert_projection(
        &old.project_boundary(&s, range).unwrap().unwrap(),
        &allocation::project(&s, &events, range),
    );

    s.revision += 1;
    s.source_seq += 1;
    s.source_revision += 1;
    s.last_observed_at += 30;
    s.allocation.cpu_millis += 17;
    let next = event(&s, 3, observed, s.last_observed_at);
    store
        .collection(super::super::super::sandbox_usage::EVENTS)
        .insert_one(super::super::super::sandbox_usage::event_document(&next).unwrap())
        .await
        .unwrap();
    events.push(next);
    store
        .collection(super::super::super::sandbox_usage::STATES)
        .replace_one(
            doc! {"_id":s.sandbox_id.as_str()},
            super::super::super::sandbox_usage::state_document(&s).unwrap(),
        )
        .await
        .unwrap();
    assert!(old.project_boundary(&s, range).unwrap().is_none());
    ready(&store, Lane::Live).await;
    let refreshed = store
        .maintain_allocation_buckets(true, Lane::Live)
        .await
        .unwrap();
    assert_eq!((refreshed.written, refreshed.source_rows), (1, 3));
    let doc = store
        .collection(COLLECTION)
        .find_one(doc! {"_id":old.key()})
        .await
        .unwrap()
        .unwrap();
    let current = DayBucket::read_boundary(&doc).unwrap();
    assert_projection(
        &current.project_boundary(&s, range).unwrap().unwrap(),
        &allocation::project(&s, &events, range),
    );
    store.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn backfill_pre_creation_days_preserves_raw_zeros_without_cache_holes() {
    let store = fixture().await;
    let today = super::super::super::usage_reports::now() / DAY * DAY;
    let s = state(1, today);
    seed(&store, std::slice::from_ref(&s)).await;
    let result = store
        .maintain_allocation_buckets(true, Lane::Backfill)
        .await
        .unwrap();
    assert_eq!((result.written, result.source_rows), (1, 0));
    let document = store
        .collection(COLLECTION)
        .find_one(doc! {})
        .await
        .unwrap()
        .unwrap();
    let bucket = DayBucket::read_boundary(&document).unwrap();
    let range = Range {
        from: today - DAY + 17,
        to: today - 67,
    };
    assert_projection(
        &bucket.project_boundary(&s, range).unwrap().unwrap(),
        &allocation::project(&s, &[], range),
    );
    store.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn closed_incomplete_history_converges_across_source_checkpoints() {
    let store = fixture().await;
    let today = super::super::super::usage_reports::now() / DAY * DAY;
    let from = today - DAY;
    let mut s = state(1, from);
    s.tracking_started_at += HOUR;
    s.complete_lifecycle = false;
    s.last_observed_at = today;
    s.revision = 2;
    let interval = event(&s, 2, from + HOUR, today);
    seed(&store, std::slice::from_ref(&s)).await;
    store
        .collection(super::super::super::sandbox_usage::EVENTS)
        .insert_one(super::super::super::sandbox_usage::event_document(&interval).unwrap())
        .await
        .unwrap();
    let first = store
        .maintain_allocation_buckets(true, Lane::Backfill)
        .await
        .unwrap();
    assert_eq!((first.written, first.source_rows), (1, 1));
    s.revision += 1;
    s.source_seq += 1;
    s.source_revision += 1;
    s.last_observed_at += 300;
    store
        .collection(super::super::super::sandbox_usage::EVENTS)
        .insert_one(
            super::super::super::sandbox_usage::event_document(&event(
                &s,
                3,
                today,
                s.last_observed_at,
            ))
            .unwrap(),
        )
        .await
        .unwrap();
    store
        .collection(super::super::super::sandbox_usage::STATES)
        .replace_one(
            doc! {"_id":s.sandbox_id.as_str()},
            super::super::super::sandbox_usage::state_document(&s).unwrap(),
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
    let next = store
        .maintain_allocation_buckets(true, Lane::Backfill)
        .await
        .unwrap();
    assert_eq!((next.reused, next.written, next.source_rows), (1, 0, 0));
    let document = store
        .collection(COLLECTION)
        .find_one(doc! {})
        .await
        .unwrap()
        .unwrap();
    let bucket = DayBucket::read(&document).unwrap();
    assert!(!bucket.total().projection.period.complete);
    assert_eq!(bucket.total().projection.period.unknown_seconds, HOUR);
    assert_projection(
        &bucket.total().projection,
        &allocation::project(&s, &[interval], Range { from, to: today }),
    );
    store.db.drop().await.unwrap();
}
