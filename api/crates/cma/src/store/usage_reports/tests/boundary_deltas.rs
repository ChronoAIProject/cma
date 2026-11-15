//! New source heads replay a prepared prefix plus only appended ledger rows.
use super::history_batch::{event, observed_store};
use super::*;
use crate::config::report_views::ViewPolicy;
use std::sync::atomic::Ordering;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn boundary_delta_matches_raw_for_new_heads_gaps_foreign_events_and_overlaps() {
    for mode in 0..4 {
        let (original, commands) = observed_store().await;
        let store = original.with_view_policy(ViewPolicy {
            allocation_rollups_enabled: true,
            ..Default::default()
        });
        store.initialize_allocation_buckets(true).await.unwrap();
        let day = (now() / 86400 - 1) * 86400;
        let mut before = state(1, day);
        let mut events: Vec<_> = (0..60).map(|n| event(&before, n, day + n * 60)).collect();
        before.revision = 60;
        before.source_seq = 60;
        before.last_observed_at = day + 3600;
        let prepared = ReportingStore::allocation_bucket_fixture_document(
            &before,
            &events,
            Range {
                from: day,
                to: day + 3600,
            },
        )
        .unwrap();
        store
            .collection(crate::store::allocation_buckets::COLLECTION)
            .insert_one(prepared)
            .await
            .unwrap();
        let mut next = before.clone();
        for n in 0..2 {
            let mut e = event(&before, 60 + n, day + 3600 + n * 600);
            e.ended_at = e.started_at + 600;
            e.allocation.cpu_millis = 137 + n as u32 * 71;
            if mode == 1 {
                e.revision += 1;
            }
            if mode == 2 && n == 0 {
                e.owner = SubjectId("foreign-fixture".into());
            }
            if mode == 3 && n == 0 {
                e.started_at -= 17;
            }
            e.quantity = e.allocation.usage(e.ended_at - e.started_at).unwrap();
            events.push(e);
        }
        next.revision = events.last().unwrap().revision;
        next.source_seq = next.revision;
        next.last_observed_at = day + 4800;
        store
            .collection("sandbox_usage_events")
            .insert_many(
                events
                    .iter()
                    .map(|e| super::super::super::sandbox_usage::event_document(e).unwrap()),
            )
            .await
            .unwrap();
        let range = Range {
            from: day + 37,
            to: day + 4913,
        };
        let expected = store
            .allocation_periods(
                &mut store.snapshot().await.unwrap(),
                std::slice::from_ref(&next),
                range,
            )
            .await
            .unwrap();
        store
            .collection("sandbox_usage_rollups")
            .delete_many(doc! {})
            .await
            .unwrap();
        commands.returned.store(0, Ordering::SeqCst);
        let actual = store
            .bucketed_allocation_range(
                &mut store.snapshot().await.unwrap(),
                std::slice::from_ref(&next),
                range,
            )
            .await
            .unwrap();
        assert_eq!(
            serde_json::to_value(&actual[&range.from]).unwrap(),
            serde_json::to_value(expected).unwrap(),
            "mode {mode}"
        );
        if mode != 3 {
            assert_eq!(
                commands.returned.load(Ordering::SeqCst),
                2,
                "only appended rows, mode {mode}"
            );
        } else {
            assert!(
                commands.returned.load(Ordering::SeqCst) > 2,
                "unsupported overlap needs raw fallback"
            );
        }
        store.db.drop().await.unwrap();
    }
}
