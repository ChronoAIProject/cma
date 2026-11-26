//! Cold report execution after separately recorded source-bucket preparation.
use super::allocated::request;
use super::bucket_ranges::{history, prepare_days};
use super::history_batch::observed_store;
use super::*;
use crate::config::report_views::{ViewPolicy, WarmScope};
use std::sync::atomic::Ordering;
use std::time::Instant;
use tracing::instrument::WithSubscriber;
const DAY: u64 = 86400;
const MEMBERS: usize = 435;
mod day_rollover;

fn checkpoint_history(
    state: &SandboxUsage,
    from: u64,
) -> Vec<layer2_core::sandbox_usage::UsageEvent> {
    // Match the configured five-minute checkpoint cadence while retaining the
    // changing allocations that expose floating-point regrouping errors.
    history(state, from, 32)
        .into_iter()
        .flat_map(|event| {
            (0..6).map(move |part| {
                let mut checkpoint = event.clone();
                checkpoint.revision = (event.revision - 1) * 6 + part + 1;
                checkpoint.source_seq = checkpoint.revision;
                checkpoint.started_at += part * 300;
                checkpoint.ended_at = checkpoint.started_at + 300;
                checkpoint.quantity = checkpoint.allocation.usage(300).unwrap();
                checkpoint
            })
        })
        .collect()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn retained_ranges_measure_435_members_at_checkpoint_cadence_with_exact_first_pages() {
    let (original, commands) = observed_store().await;
    let fixture_at = now();
    let from = (fixture_at / DAY - 31) * DAY;
    let to = fixture_at / 3600 * 3600;
    let prepared = Instant::now();
    original.initialize_allocation_buckets(true).await.unwrap();
    let mut states = Vec::new();
    let mut tail_events = Vec::new();
    let mut prepared_to = if to.is_multiple_of(DAY) {
        to + 3600
    } else {
        to
    };
    let mut source_count = 0;
    for id in 0..MEMBERS {
        let mut s = state(id, from);
        let mut events = checkpoint_history(&s, from);
        events.retain(|e| e.ended_at <= to);
        s.revision = events.last().unwrap().revision;
        s.source_seq = s.revision;
        s.last_observed_at = to;
        source_count += events.len();
        original
            .collection("sandbox_usage_events")
            .insert_many(
                events
                    .iter()
                    .map(|e| super::super::super::sandbox_usage::event_document(e).unwrap()),
            )
            .await
            .unwrap();
        // At UTC midnight the live worker prepares the first hour's tail
        // inputs even before a complete hour exists. Exercise that same case.
        prepare_days(&original, &s, &events, from, prepared_to).await;
        tail_events.push(
            events
                .into_iter()
                .filter(|event| event.ended_at > to / DAY * DAY)
                .collect(),
        );
        states.push(s);
    }
    seed(&original, &states).await;
    for n in 0..12 {
        super::owner_names::name(
            &original,
            &format!("owner-{n}"),
            &format!("Public person {n}"),
        )
        .await;
    }
    original
        .collection("sandbox_usage_events")
        .create_index(
            mongodb::IndexModel::builder()
                .keys(doc! {"sandbox_id":1,"revision":1})
                .build(),
        )
        .await
        .unwrap();
    eprintln!("range_fixture members={MEMBERS} checkpoint_seconds=300 source_events={source_count} preparation_ms={} preparation=production_projector_direct_seed worker_execution=false",prepared.elapsed().as_millis());
    for (bucketed, lagging, principal) in [
        (false, false, "raw-fixture"),
        (true, false, "bucket-fixture"),
        (true, true, "lagging-fixture"),
    ] {
        if lagging {
            advance_heads(&original, &mut states).await;
        }
        let store = original.with_view_policy(ViewPolicy {
            enabled: true,
            allocation_rollups_enabled: bucketed,
            view_scopes: vec![WarmScope {
                principal: principal.into(),
                admin: true,
                sandbox_id: None,
            }],
            ..Default::default()
        });
        for window in ["24h", "7d", "30d"] {
            // Each range starts without any immutable report or legacy cache;
            // the source buckets are present only for the explicitly enabled path.
            store
                .collection("sandbox_usage_rollups")
                .delete_many(doc! {})
                .await
                .unwrap();
            commands.event_reads.store(0, Ordering::SeqCst);
            let began = Instant::now();
            let path = format!("/api/v1/admin/usage/metrics/view?window={window}");
            let (status, pending) = request(
                &store,
                "GET",
                &path,
                principal,
                true,
                serde_json::Value::Null,
            )
            .await;
            assert_eq!(status, 202, "{pending}");
            let admission_us = began.elapsed().as_micros();
            let lease = store
                .claim_report("report:0", "range-measurement")
                .await
                .unwrap()
                .unwrap();
            assert_eq!(pending["report"]["report_id"], lease.report.report_id);
            let tail_preparation = Instant::now();
            if bucketed && !lagging {
                day_rollover::prepare_report_tail(
                    &store,
                    &states,
                    &tail_events,
                    &mut prepared_to,
                    lease.report.effective_range.to,
                )
                .await;
            }
            let tail_preparation_us = tail_preparation.elapsed().as_micros();
            let generation = Instant::now();
            let subscriber = tracing_subscriber::fmt()
                .with_max_level(tracing::Level::INFO)
                .with_writer(std::io::stderr)
                .finish();
            let result = crate::sandbox_metrics::generate_fixture_with_collection(
                store.clone(),
                &lease,
                false,
            )
            .with_subscriber(subscriber)
            .await;
            let generation_us = generation.elapsed().as_micros();
            let success = result.is_ok();
            if !success && bucketed {
                panic!("prepared range {window} failed: {result:?}");
            }
            let report = result.unwrap_or_else(|_| lease.report.clone());
            let finalize = Instant::now();
            store.finish_report(&lease, report, success).await.unwrap();
            let finalization_us = finalize.elapsed().as_micros();
            let event_reads = commands.event_reads.load(Ordering::SeqCst);
            if bucketed && !lagging {
                assert_eq!(event_reads, 0, "prepared {window} reread raw history");
            }
            if !success {
                eprintln!("range_measurement window={window} bucketed={bucketed} lagging={lagging} status=failed admission_us={admission_us} generation_us={generation_us} finalization_us={finalization_us} event_reads={event_reads}");
                continue;
            }
            let reads = Instant::now();
            let (status, view) = request(
                &store,
                "GET",
                &path,
                principal,
                true,
                serde_json::Value::Null,
            )
            .await;
            assert_eq!(status, 200, "{view}");
            let id = view["report"]["report_id"].as_str().unwrap();
            assert_eq!(id, lease.report.report_id);
            assert!(view["generated_at"].is_number());
            let (status, page) = request(
                &store,
                "GET",
                &format!("/api/v1/admin/usage/metrics/reports/{id}/sandboxes?limit=50"),
                principal,
                true,
                serde_json::Value::Null,
            )
            .await;
            assert_eq!(status, 200, "{page}");
            let rows = page["items"].as_array().unwrap();
            assert_eq!(rows.len(), 50);
            assert!(rows.iter().all(|r| r["owner"]["display_name"].is_string()));
            assert_eq!(page["total_count"], MEMBERS);
            assert_eq!(view["report"]["summary"]["sandbox_count"], MEMBERS);
            eprintln!("range_measurement window={window} bucketed={bucketed} lagging={lagging} status=ready admission_us={admission_us} tail_preparation_us={tail_preparation_us} generation_us={generation_us} finalization_us={finalization_us} first_page_us={} committed_reads_us={} event_reads={event_reads} rows=50 labels=50 summary_bytes={} page_bytes={}",began.elapsed().as_micros(),reads.elapsed().as_micros(),serde_json::to_vec(&view).unwrap().len(),serde_json::to_vec(&page).unwrap().len());
        }
    }
    store_cleanup(&original).await;
}
async fn advance_heads(store: &ReportingStore, states: &mut [SandboxUsage]) {
    let at = now();
    for state in states {
        let mut event = super::history_batch::event(state, state.revision, state.last_observed_at);
        event.ended_at = at;
        event.quantity = event.allocation.usage(at - event.started_at).unwrap();
        state.last_observed_at = at;
        state.revision = event.revision;
        state.source_seq = event.source_seq;
        store
            .collection("sandbox_usage_events")
            .insert_one(super::super::super::sandbox_usage::event_document(&event).unwrap())
            .await
            .unwrap();
        store
            .collection("sandbox_usage")
            .replace_one(
                doc! {"_id":state.sandbox_id.as_str()},
                super::super::super::sandbox_usage::state_document(state).unwrap(),
            )
            .await
            .unwrap();
    }
}
async fn store_cleanup(store: &ReportingStore) {
    store.db.drop().await.unwrap();
}
