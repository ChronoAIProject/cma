//! Cold-range reuse must equal daily reads and preserve snapshot/error authority.
use super::*;
use crate::store::sandbox_usage::event_document;
use std::collections::BTreeMap;

const DAY: u64 = 86_400;
const CACHE: &str = "sandbox_usage_rollups";
const EVENTS: &str = "sandbox_usage_events";

async fn store(variable: &str) -> Arc<ReportingStore> {
    let database = match std::env::var(variable) {
        Ok(name) => name,
        Err(std::env::VarError::NotPresent) => {
            format!("metrics_2140_{}", uuid::Uuid::new_v4().simple())
        }
        Err(error) => panic!("invalid named disposable database: {error}"),
    };
    assert!(database.starts_with("metrics_2140_"));
    ReportingStore::connect(
        &std::env::var("CHRONO_TEST_MONGODB_URI").expect("disposable replica set required"),
        &database,
        &[19; 32],
    )
    .await
    .unwrap()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn allocation_cold_range_native_matches_daily_and_reuses_valid_cache() {
    let store = store("CHRONO_TEST_ALLOCATION_DB_01").await;
    let day = (now() / DAY - 32) * DAY;
    let mut states = Vec::new();
    // Nine members crosses the eight-ID raw-group boundary. Each member has
    // more than one cursor batch over the largest range.
    for id in 0..9 {
        let mut state = state(id, day);
        state.last_observed_at = day + 31 * DAY;
        state.revision = 31 * 96;
        let documents = (0..31 * 96)
            .rev()
            .map(|n| {
                let mut event = history_batch::event(&state, n, day + n * 900);
                event.ended_at = event.started_at + 900;
                event.quantity = event.allocation.usage(900).unwrap();
                event_document(&event).unwrap()
            })
            .collect::<Vec<_>>();
        store
            .collection(EVENTS)
            .insert_many(documents)
            .await
            .unwrap();
        states.push(state);
    }
    store
        .collection(EVENTS)
        .create_index(
            mongodb::IndexModel::builder()
                .keys(doc! {"sandbox_id":1,"ended_at":1,"started_at":1,"revision":1})
                .build(),
        )
        .await
        .unwrap();
    for days in [1, 7, 30] {
        store.collection(CACHE).delete_many(doc! {}).await.unwrap();
        let range = Range {
            from: day + 123,
            to: day + days * DAY + 123,
        };
        let mut snapshot = store.snapshot().await.unwrap();
        let prepared = store
            .cold_allocation_range(&mut snapshot, &states, range)
            .await
            .unwrap();
        if days == 1 {
            assert!(prepared.is_none());
            continue;
        }
        let prepared = prepared.unwrap();
        let ids = states
            .iter()
            .map(|s| s.sandbox_id.as_str())
            .collect::<Vec<_>>();
        let mut from = range.from;
        while from < range.to {
            let slice = Range {
                from,
                to: ((from / DAY + 1) * DAY).min(range.to),
            };
            let raw = store
                .allocation_events(&mut snapshot, &ids, slice)
                .await
                .unwrap();
            let expected = states
                .iter()
                .map(|state| {
                    (
                        state.sandbox_id.to_string(),
                        allocation::project(state, &raw[state.sandbox_id.as_str()], slice),
                    )
                })
                .collect::<BTreeMap<_, _>>();
            assert_eq!(
                serde_json::to_value(&prepared[&from]).unwrap(),
                serde_json::to_value(expected).unwrap()
            );
            let cached = store
                .allocation_periods(&mut store.snapshot().await.unwrap(), &states, slice)
                .await
                .unwrap();
            assert_eq!(
                serde_json::to_value(cached).unwrap(),
                serde_json::to_value(&prepared[&from]).unwrap()
            );
            from = slice.to;
        }
        assert!(store
            .cold_allocation_range(&mut store.snapshot().await.unwrap(), &states, range)
            .await
            .unwrap()
            .is_some());
        store.collection(CACHE).delete_many(doc! {}).await.unwrap();
        // A malformed matching record remains fail-closed; it must not be
        // treated as a cache miss and silently repaired.
        store.collection(CACHE).insert_one(doc! {"_id":"invalid","sandbox_id":states[0].sandbox_id.as_str(),"day":day as i64,"record":"invalid"}).await.unwrap();
        assert!(matches!(
            store
                .cold_allocation_range(&mut store.snapshot().await.unwrap(), &states, range)
                .await,
            Err(MetricsError::Unavailable)
        ));
        assert!(store
            .allocation_periods(
                &mut store.snapshot().await.unwrap(),
                &states,
                Range {
                    from: range.from,
                    to: day + DAY
                }
            )
            .await
            .is_err());
    }
    store.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn allocation_cold_range_native_snapshot_and_late_corruption_are_not_declines() {
    let store = store("CHRONO_TEST_ALLOCATION_DB_02").await;
    let day = (now() / DAY - 4) * DAY;
    let range = Range {
        from: day,
        to: day + 3 * DAY,
    };
    let mut state = state(0, day);
    state.last_observed_at = range.to;
    state.revision = 3 * 1440;
    let events = (0..3 * 1440)
        .map(|n| event_document(&history_batch::event(&state, n, day + n * 60)).unwrap())
        .collect::<Vec<_>>();
    store.collection(EVENTS).insert_many(events).await.unwrap();
    let states = [state];
    // Prime all three slices, then remove one valid row. The pinned snapshot
    // should reuse the two rows it can validate and repair only the missing
    // slice from its own event view.
    store
        .cold_allocation_range(&mut store.snapshot().await.unwrap(), &states, range)
        .await
        .unwrap()
        .unwrap();
    store
        .collection(CACHE)
        .delete_one(doc! {"_id":format!("{}:{}", states[0].sandbox_id, day + DAY)})
        .await
        .unwrap();
    let mut old = store.snapshot().await.unwrap();
    store
        .allocation_events(
            &mut old,
            &[states[0].sandbox_id.as_str()],
            Range {
                from: day,
                to: day + DAY,
            },
        )
        .await
        .unwrap();
    // The cache precheck and raw cursor must both use this already pinned view.
    store
        .collection(EVENTS)
        .update_one(
            doc! {"revision":4310_i64},
            doc! {"$set":{"owner":"corrupt"}},
        )
        .await
        .unwrap();
    let prepared = store
        .cold_allocation_range(&mut old, &states, range)
        .await
        .unwrap()
        .unwrap();
    assert!(prepared
        .values()
        .flat_map(|p| p.values())
        .all(|p| p.period.complete));
    assert_eq!(
        store
            .collection(CACHE)
            .count_documents(doc! {})
            .await
            .unwrap(),
        3
    );
    store.collection(CACHE).delete_many(doc! {}).await.unwrap();
    assert!(matches!(
        store
            .cold_allocation_range(&mut store.snapshot().await.unwrap(), &states, range)
            .await,
        Err(MetricsError::Unavailable)
    ));
    assert_eq!(
        store
            .collection(CACHE)
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    // Pool circuit rejection must not silently turn into a daily-path retry.
    let mut snapshot = store.snapshot().await.unwrap();
    store
        .circuit
        .store(now() + 30, std::sync::atomic::Ordering::SeqCst);
    assert!(matches!(
        store
            .cold_allocation_range(&mut snapshot, &states, range)
            .await,
        Err(MetricsError::Busy)
    ));
    store.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn allocation_cold_range_native_combined_cap_declines_without_cache_prefix() {
    let store = store("CHRONO_TEST_ALLOCATION_DB_01").await;
    let day = (now() / DAY - 4) * DAY;
    let range = Range {
        from: day,
        to: day + 3 * DAY,
    };
    let mut state = state(0, day);
    state.last_observed_at = day + 200_002;
    state.revision = 100_001;
    for start in (0..100_001).step_by(2000) {
        let documents = (start..(start + 2000).min(100_001))
            .map(|n| {
                let mut event = history_batch::event(&state, n, day + n * 2);
                event.ended_at = event.started_at + 2;
                event.quantity = event.allocation.usage(2).unwrap();
                event_document(&event).unwrap()
            })
            .collect::<Vec<_>>();
        store
            .collection(EVENTS)
            .insert_many(documents)
            .await
            .unwrap();
    }
    store
        .collection(EVENTS)
        .create_index(
            mongodb::IndexModel::builder()
                .keys(doc! {"sandbox_id":1,"ended_at":1,"started_at":1,"revision":1})
                .build(),
        )
        .await
        .unwrap();
    let states = [state];
    let mut snapshot = store.snapshot().await.unwrap();
    assert!(store
        .cold_allocation_range(&mut snapshot, &states, range)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        store
            .collection(CACHE)
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    // The original day contract still admits each bounded day and supplies the
    // caller's single fallback without recursion or partial prepared values.
    for offset in 0..3 {
        let slice = Range {
            from: day + offset * DAY,
            to: day + (offset + 1) * DAY,
        };
        let daily = store
            .allocation_periods(&mut snapshot, &states, slice)
            .await
            .unwrap();
        assert!(daily[states[0].sandbox_id.as_str()].period.complete);
        assert_eq!(
            daily[states[0].sandbox_id.as_str()].period.uptime_seconds,
            DAY as f64
        );
    }
    store.db.drop().await.unwrap();
}
