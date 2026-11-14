//! Supported ledger recovery invalidates prepared coverage and can rebuild it.
use super::allocated::request;
use super::history_batch::{event, observed_store};
use super::*;
use crate::config::report_views::ViewPolicy;
use crate::store::allocation_buckets::{Lane, COLLECTION};
use std::sync::atomic::Ordering;
const DAY: u64 = 86400;
const CONTROL_KEY: &str = "allocation-buckets-v1:backfill";

async fn report(
    original: &Arc<ReportingStore>,
    enabled: bool,
    principal: &str,
    range: Range,
) -> (serde_json::Value, serde_json::Value) {
    let store = original.with_view_policy(ViewPolicy {
        allocation_rollups_enabled: enabled,
        ..Default::default()
    });
    let (status, pending) = request(
        &store,
        "POST",
        "/api/v1/admin/usage/metrics/reports",
        principal,
        true,
        serde_json::json!({"from":range.from,"to":range.to,"step_seconds":60}),
    )
    .await;
    assert_eq!(status, 202, "{pending}");
    let lease = store
        .claim_report("report:0", principal)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(pending["report_id"], lease.report.report_id);
    let generated =
        crate::sandbox_metrics::generate_fixture_with_collection(store.clone(), &lease, false)
            .await
            .unwrap();
    let summary = serde_json::to_value(&generated.summary).unwrap();
    store.finish_report(&lease, generated, true).await.unwrap();
    let (status, page) = request(
        &store,
        "GET",
        &format!(
            "/api/v1/admin/usage/metrics/reports/{}/sandboxes?limit=50",
            lease.report.report_id
        ),
        principal,
        true,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, 200, "{page}");
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    (summary, page["items"].clone())
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn rebaseline_falls_back_then_rebuilds_reports_and_advances_maintenance() {
    let (original, commands) = observed_store().await;
    let store = original.with_view_policy(ViewPolicy {
        allocation_rollups_enabled: true,
        ..Default::default()
    });
    store.initialize_allocation_buckets(true).await.unwrap();
    let day = (now() / DAY - 1) * DAY;
    let mut before = state(1, day);
    let mut events: Vec<_> = (0..10).map(|n| event(&before, n, day + n * 60)).collect();
    before.revision = 10;
    before.source_seq = 10;
    before.last_observed_at = day + 600;
    let prepared = ReportingStore::allocation_bucket_fixture_document(
        &before,
        &events,
        Range {
            from: day,
            to: day + DAY,
        },
    )
    .unwrap();
    store
        .collection(COLLECTION)
        .insert_one(&prepared)
        .await
        .unwrap();

    let at = |seconds| {
        layer2_core::rfc3339_at(std::time::UNIX_EPOCH + std::time::Duration::from_secs(seconds))
    };
    let mut sandbox =
        crate::store::sandbox_mongo_integration_tests::sandbox(before.sandbox_id.as_str());
    sandbox.owner = before.owner.clone();
    sandbox.created_at = at(day);
    sandbox.status = layer2_core::sandbox::SandboxStatus::Paused;
    sandbox.revision = before.source_revision + 1;
    let (next, baseline) = layer2_core::sandbox_usage::rebaseline(
        &before,
        &sandbox,
        before.source_seq + 1,
        &at(day + 900),
    )
    .unwrap()
    .unwrap();
    assert!(before.complete_lifecycle);
    assert!(!next.complete_lifecycle);
    assert!(next.revision > before.revision);
    assert_eq!(
        baseline.kind,
        layer2_core::sandbox_usage::UsageEventKind::Baseline
    );
    events.push(baseline);
    seed(&store, std::slice::from_ref(&next)).await;
    store
        .collection("sandbox_usage_events")
        .insert_many(
            events
                .iter()
                .map(|e| crate::store::sandbox_usage::event_document(e).unwrap()),
        )
        .await
        .unwrap();
    let range = Range {
        from: day + 60,
        to: day + 3600,
    };

    // Neither a same-revision completeness mutation nor foreign ownership is
    // allowed to masquerade as supported recovery. The lane must not advance.
    let checkpoint = store
        .collection(CONTROL)
        .find_one(doc! {"_id":CONTROL_KEY})
        .await
        .unwrap()
        .unwrap();
    for invalid in [
        SandboxUsage {
            revision: before.revision,
            ..next.clone()
        },
        SandboxUsage {
            owner: SubjectId("foreign-fixture".into()),
            ..next.clone()
        },
    ] {
        store
            .collection("sandbox_usage")
            .replace_one(
                doc! {"_id":next.sandbox_id.as_str()},
                crate::store::sandbox_usage::state_document(&invalid).unwrap(),
            )
            .await
            .unwrap();
        assert!(store
            .bucketed_allocation_range(
                &mut store.snapshot().await.unwrap(),
                std::slice::from_ref(&invalid),
                range,
            )
            .await
            .is_err());
        assert!(store
            .maintain_allocation_buckets(true, Lane::Backfill)
            .await
            .is_err());
        let row = store
            .collection(CONTROL)
            .find_one(doc! {"_id":CONTROL_KEY})
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.get("record"), checkpoint.get("record"));
        assert_eq!(
            store
                .collection(COLLECTION)
                .find_one(doc! {})
                .await
                .unwrap()
                .unwrap(),
            prepared
        );
        // Test-only lease expiry lets the next assertion retry the same page.
        store
            .collection(CONTROL)
            .update_one(
                doc! {"_id":CONTROL_KEY},
                doc! {"$set":{
                    "lease_until":mongodb::bson::DateTime::from_millis(0),
                    "next_at":mongodb::bson::DateTime::from_millis(0),
                }},
            )
            .await
            .unwrap();
    }
    store
        .collection("sandbox_usage")
        .replace_one(
            doc! {"_id":next.sandbox_id.as_str()},
            crate::store::sandbox_usage::state_document(&next).unwrap(),
        )
        .await
        .unwrap();
    let raw = report(&store, false, "raw-rebaseline", range).await;
    assert_eq!(raw.0["allocated_period"]["complete"], false);
    store
        .collection("sandbox_usage_rollups")
        .delete_many(doc! {})
        .await
        .unwrap();
    commands.event_reads.store(0, Ordering::SeqCst);
    let fallback = report(&store, true, "fallback-rebaseline", range).await;
    assert_eq!(fallback, raw);
    assert!(commands.event_reads.load(Ordering::SeqCst) > 0);

    let rebuilt = store
        .maintain_allocation_buckets(true, Lane::Backfill)
        .await
        .unwrap();
    assert_eq!((rebuilt.states, rebuilt.written, rebuilt.reused), (1, 1, 0));
    let saved = store
        .collection(COLLECTION)
        .find_one(doc! {})
        .await
        .unwrap()
        .unwrap();
    let record = saved.get_document("record").unwrap();
    assert!(!record.get_bool("complete_lifecycle").unwrap());
    assert_eq!(saved.get_i64("revision").unwrap(), next.revision as i64);
    let checkpoint = store
        .collection(CONTROL)
        .find_one(doc! {"_id":CONTROL_KEY})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        checkpoint
            .get_document("record")
            .unwrap()
            .get_i64("day")
            .unwrap(),
        (day - DAY) as i64
    );
    store
        .collection("sandbox_usage_rollups")
        .delete_many(doc! {})
        .await
        .unwrap();
    commands.event_reads.store(0, Ordering::SeqCst);
    assert_eq!(report(&store, true, "rebuilt-rebaseline", range).await, raw);
    assert_eq!(commands.event_reads.load(Ordering::SeqCst), 0);
    store.db.drop().await.unwrap();
}
