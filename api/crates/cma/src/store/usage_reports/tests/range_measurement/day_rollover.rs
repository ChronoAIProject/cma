//! Keep the measured fixture prepared through the range selected at admission.
use super::*;
use layer2_core::sandbox_usage::UsageEvent;
use std::collections::BTreeMap;

pub(super) async fn prepare_report_tail(
    store: &ReportingStore,
    states: &[SandboxUsage],
    tail_events: &[Vec<UsageEvent>],
    prepared_to: &mut u64,
    report_to: u64,
) {
    let to = report_to.div_ceil(3600) * 3600;
    if to <= *prepared_to {
        return;
    }
    assert_eq!(states.len(), tail_events.len());
    let mut from = *prepared_to / DAY * DAY;
    while from < to {
        let range = Range {
            from,
            to: (from + DAY).min(to),
        };
        for (state, events) in states.iter().zip(tail_events) {
            assert!(state.last_observed_at <= *prepared_to);
            let own: Vec<_> = events
                .iter()
                .filter(|event| event.ended_at > from && event.started_at < range.to)
                .cloned()
                .collect();
            let document =
                ReportingStore::allocation_bucket_fixture_document(state, &own, range).unwrap();
            store
                .collection(crate::store::allocation_buckets::COLLECTION)
                .replace_one(
                    doc! {"sandbox_id":state.sandbox_id.as_str(), "day":from as i64},
                    document,
                )
                .upsert(true)
                .await
                .unwrap();
        }
        from = range.to;
    }
    *prepared_to = to;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn admitted_ranges_cross_midnight_without_raw_fallback_or_changed_totals() {
    let (original, commands) = observed_store().await;
    let store = original.with_view_policy(ViewPolicy {
        allocation_rollups_enabled: true,
        ..Default::default()
    });
    store.initialize_allocation_buckets(true).await.unwrap();
    let from = (now() / DAY - 2) * DAY;
    let head = from + 23 * 3600;
    let mut states = Vec::new();
    let mut tails = Vec::new();
    for id in 0..3 {
        let mut state = state(id, from);
        let mut events = history(&state, from, 1);
        events.retain(|event| event.ended_at <= head);
        state.revision = events.last().unwrap().revision;
        state.source_seq = state.revision;
        state.last_observed_at = head;
        store
            .collection("sandbox_usage_events")
            .insert_many(events.iter().map(|event| {
                super::super::super::super::sandbox_usage::event_document(event).unwrap()
            }))
            .await
            .unwrap();
        prepare_days(&store, &state, &events, from, head).await;
        states.push(state);
        tails.push(events);
    }
    let mut prepared_to = head;
    for to in [from + DAY, from + DAY + 60, from + 2 * DAY + 60] {
        let range = Range { from, to };
        let mut expected = BTreeMap::new();
        let mut session = store.snapshot().await.unwrap();
        let mut start = range.from;
        while start < to {
            let end = ((start / DAY + 1) * DAY).min(to);
            expected.insert(
                start,
                store
                    .allocation_periods(
                        &mut session,
                        &states,
                        Range {
                            from: start,
                            to: end,
                        },
                    )
                    .await
                    .unwrap(),
            );
            start = end;
        }
        store
            .collection("sandbox_usage_rollups")
            .delete_many(doc! {})
            .await
            .unwrap();
        if to >= from + DAY {
            commands.event_reads.store(0, Ordering::SeqCst);
            store
                .bucketed_allocation_range(&mut store.snapshot().await.unwrap(), &states, range)
                .await
                .unwrap();
            assert!(
                commands.event_reads.load(Ordering::SeqCst) > 0,
                "missing preparation must reproduce raw fallback"
            );
            store
                .collection("sandbox_usage_rollups")
                .delete_many(doc! {})
                .await
                .unwrap();
        }
        prepare_report_tail(&store, &states, &tails, &mut prepared_to, to).await;
        commands.event_reads.store(0, Ordering::SeqCst);
        let got = store
            .bucketed_allocation_range(&mut store.snapshot().await.unwrap(), &states, range)
            .await
            .unwrap();
        assert_eq!(
            serde_json::to_value(got).unwrap(),
            serde_json::to_value(expected).unwrap()
        );
        assert_eq!(commands.event_reads.load(Ordering::SeqCst), 0, "{range:?}");
    }
    store.db.drop().await.unwrap();
}
