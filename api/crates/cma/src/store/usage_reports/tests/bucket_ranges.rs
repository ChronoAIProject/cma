//! Reader parity uses the same immutable ledger through both independent paths.
use super::history_batch::{event, observed_store};
use super::*;
use crate::config::report_views::ViewPolicy;
use std::collections::BTreeMap;
use std::sync::atomic::Ordering;
const DAY: u64 = 86400;
const BUCKETS: &str = crate::store::allocation_buckets::COLLECTION;

pub(super) fn history(
    state: &SandboxUsage,
    start: u64,
    days: u64,
) -> Vec<layer2_core::sandbox_usage::UsageEvent> {
    (0..days * 48)
        .map(|n| {
            let mut e = event(state, n, start + n * 1800);
            e.ended_at = e.started_at + 1800;
            // Non-power-of-two rates make order-changing float folds observable.
            e.allocation.cpu_millis = 137 + ((n % 11) * 71) as u32;
            e.allocation.memory_mib = 733 + ((n % 7) * 113) as u32;
            e.allocation.disk_gib = 19 + (n % 3) as u32;
            e.allocation.running = n % 5 != 0;
            e.quantity = e.allocation.usage(1800).unwrap();
            e
        })
        .collect()
}

pub(super) async fn prepare_days(
    store: &ReportingStore,
    state: &SandboxUsage,
    events: &[layer2_core::sandbox_usage::UsageEvent],
    start: u64,
    to: u64,
) {
    let mut docs = Vec::new();
    let mut from = start;
    while from < to {
        let range = Range {
            from,
            to: (from + DAY).min(to),
        };
        let own: Vec<_> = events
            .iter()
            .filter(|e| e.ended_at > range.from && e.started_at < range.to)
            .cloned()
            .collect();
        docs.push(ReportingStore::allocation_bucket_fixture_document(state, &own, range).unwrap());
        from = range.to;
    }
    store.collection(BUCKETS).insert_many(docs).await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn bucket_ranges_equal_raw_for_all_presets_edges_and_live_tails() {
    let (original, commands) = observed_store().await;
    let store = original.with_view_policy(ViewPolicy {
        allocation_rollups_enabled: true,
        ..Default::default()
    });
    store.initialize_allocation_buckets(true).await.unwrap();
    let start = (now() / DAY - 31) * DAY;
    let end = start + 30 * DAY;
    let mut states = Vec::new();
    for id in 0..3 {
        let mut s = state(id, start);
        s.revision = 30 * 48;
        s.last_observed_at = end;
        let events = history(&s, start, 30);
        store
            .collection("sandbox_usage_events")
            .insert_many(
                events
                    .iter()
                    .map(|e| super::super::super::sandbox_usage::event_document(e).unwrap()),
            )
            .await
            .unwrap();
        prepare_days(&store, &s, &events, start, end).await;
        states.push(s);
    }
    for range in [
        // Finalization can leave no eligible seconds. The raw report still
        // publishes its explicit no-finalized-samples state in this case.
        Range { from: end, to: end },
        Range {
            from: end - DAY,
            to: end,
        },
        Range {
            from: end - 7 * DAY,
            to: end,
        },
        Range {
            from: start,
            to: end,
        },
        Range {
            from: start + 67,
            to: end - 113,
        },
        Range {
            from: end - 7 * DAY + 301,
            to: end + 301,
        },
        Range {
            from: end - DAY + 3600,
            to: end - DAY + 7200,
        },
    ] {
        let mut expected = BTreeMap::new();
        let mut session = store.snapshot().await.unwrap();
        let mut from = range.from;
        while from < range.to {
            let to = ((from / DAY + 1) * DAY).min(range.to);
            expected.insert(
                from,
                store
                    .allocation_periods(&mut session, &states, Range { from, to })
                    .await
                    .unwrap(),
            );
            from = to;
        }
        // Do not let the legacy exact-range cache mask raw fallback in the new path.
        store
            .collection("sandbox_usage_rollups")
            .delete_many(doc! {})
            .await
            .unwrap();
        commands.event_reads.store(0, Ordering::SeqCst);
        let got = store
            .bucketed_allocation_range(&mut store.snapshot().await.unwrap(), &states, range)
            .await
            .unwrap();
        assert_eq!(
            serde_json::to_value(&got).unwrap(),
            serde_json::to_value(expected).unwrap(),
            "{range:?}"
        );
        let deadlines = std::mem::take(&mut *commands.bucket_find_deadlines_ms.lock().unwrap());
        let expected_queries = if range.from == range.to {
            0
        } else if range.from.is_multiple_of(DAY) && range.to.is_multiple_of(DAY) {
            1
        } else {
            2
        };
        assert_eq!(deadlines, vec![Some(5_000); expected_queries], "{range:?}");
        if range.to <= end {
            assert_eq!(commands.event_reads.load(Ordering::SeqCst), 0);
        }
    }
    // A later source revision can reuse complete closed history, but future
    // cache evidence cannot substitute for an earlier report snapshot.
    let mut advanced = states.clone();
    for s in &mut advanced {
        s.revision += 1;
        s.last_observed_at += 60;
    }
    let full = Range {
        from: start,
        to: end,
    };
    commands.event_reads.store(0, Ordering::SeqCst);
    store
        .bucketed_allocation_range(&mut store.snapshot().await.unwrap(), &advanced, full)
        .await
        .unwrap();
    assert_eq!(commands.event_reads.load(Ordering::SeqCst), 0);
    let mut drift = states.clone();
    drift[0].allocation.cpu_millis += 1;
    assert!(store
        .bucketed_allocation_range(&mut store.snapshot().await.unwrap(), &drift, full)
        .await
        .is_err());
    store
        .collection(BUCKETS)
        .update_one(
            doc! {"sandbox_id":states[0].sandbox_id.as_str()},
            doc! {"$set":{"summary_record":mongodb::bson::Binary {
                subtype: mongodb::bson::spec::BinarySubtype::Generic,
                bytes: vec![0, 0, 0, 0],
            }}},
        )
        .await
        .unwrap();
    assert!(store
        .bucketed_allocation_range(&mut store.snapshot().await.unwrap(), &states, full)
        .await
        .is_err());
    store.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn disabled_bucket_reader_refuses_before_touching_corrupt_cache() {
    let store = fixture().await;
    store
        .collection(BUCKETS)
        .insert_one(doc! {"document_schema_version":"invalid"})
        .await
        .unwrap();
    let from = now() - DAY;
    assert!(matches!(
        store
            .bucketed_allocation_range(
                &mut store.snapshot().await.unwrap(),
                &[state(0, from)],
                Range {
                    from,
                    to: from + DAY
                }
            )
            .await,
        Err(MetricsError::ViewDisabled)
    ));
    store.db.drop().await.unwrap();
}
