use super::*;
use crate::store::sandbox_usage::{event_document, state_document, STATES};

const DAY: u64 = 86_400;
const CACHE: &str = "sandbox_usage_rollups";

async fn cache(store: &ReportingStore) -> Vec<Document> {
    store
        .collection(CACHE)
        .find(doc! {})
        .sort(doc! {"_id":1})
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap()
}

async fn project(
    store: &ReportingStore,
    session: &mut mongodb::ClientSession,
    state: &SandboxUsage,
    range: Range,
    cold: bool,
) -> f64 {
    if cold {
        store
            .cold_allocation_range(session, std::slice::from_ref(state), range)
            .await
            .unwrap()
            .unwrap()
            .values()
            .map(|rows| rows[state.sandbox_id.as_str()].period.cpu_core_hours)
            .sum()
    } else {
        store
            .allocation_periods(session, std::slice::from_ref(state), range)
            .await
            .unwrap()[state.sandbox_id.as_str()]
        .period
        .cpu_core_hours
    }
}

async fn late_snapshot(cold: bool) {
    let store = fixture().await;
    let day = (now() / DAY - 4) * DAY;
    let range = Range {
        from: day,
        to: day + if cold { 3 * DAY } else { DAY },
    };
    let old = state(0, day);
    seed(&store, std::slice::from_ref(&old)).await;
    let mut snapshot = store.snapshot().await.unwrap();
    store
        .collection(STATES)
        .find_one(doc! {"_id":old.sandbox_id.as_str()})
        .session(&mut snapshot)
        .await
        .unwrap()
        .unwrap();
    let mut new = old.clone();
    new.revision += 1;
    new.last_observed_at += 60;
    new.allocation.cpu_millis = 4000;
    store
        .collection("sandbox_usage_events")
        .insert_one(event_document(&history_batch::event(&old, 1, day)).unwrap())
        .await
        .unwrap();
    store
        .collection(STATES)
        .replace_one(
            doc! {"_id":old.sandbox_id.as_str()},
            state_document(&new).unwrap(),
        )
        .await
        .unwrap();
    let current = project(
        &store,
        &mut store.snapshot().await.unwrap(),
        &new,
        range,
        cold,
    )
    .await;
    let saved = cache(&store).await;
    assert_eq!(saved.len(), if cold { 3 } else { 1 });
    for row in &saved {
        assert_eq!(
            row.get_document("record")
                .unwrap()
                .get_i64("source_watermark")
                .unwrap(),
            2
        );
    }
    let previous = project(&store, &mut snapshot, &old, range, cold).await;
    assert!(current > previous);
    assert_eq!(
        cache(&store).await,
        saved,
        "older snapshot replaced newer cache"
    );

    // Equal revisions with a different requested range must not replace the winner.
    let other_range = Range {
        from: range.from + 30,
        ..range
    };
    project(
        &store,
        &mut store.snapshot().await.unwrap(),
        &new,
        other_range,
        cold,
    )
    .await;
    assert_eq!(cache(&store).await, saved);

    // A strictly newer revision can refresh that same cache key.
    new.revision += 1;
    project(
        &store,
        &mut store.snapshot().await.unwrap(),
        &new,
        other_range,
        cold,
    )
    .await;
    let refreshed = cache(&store).await;
    assert_eq!(
        refreshed[0]
            .get_document("record")
            .unwrap()
            .get_i64("source_watermark")
            .unwrap(),
        3
    );
    store.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn daily_rollups_do_not_regress_after_an_older_snapshot_finishes() {
    late_snapshot(false).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn cold_rollups_do_not_regress_after_an_older_snapshot_finishes() {
    late_snapshot(true).await;
}
