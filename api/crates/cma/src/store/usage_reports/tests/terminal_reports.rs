use super::allocated::{measured_fixture, request};
use super::*;
use crate::store::{sandbox_mongo::MongoSandboxStore, sandbox_usage};
use layer2_core::sandbox::{
    store::{PageRequest, SandboxEventStore, SandboxStore},
    RuntimeBinding, SandboxEventKind, SandboxHome, SandboxStatus, TypedFailure,
};
use layer2_core::sandbox_usage::UsageEventKind;
use layer2_core::sandbox_usage_service::SandboxUsageStore;
use layer3_adapter::{HomeVolumePolicy, HomeVolumeReceipt};
use serde_json::{json, Value};

mod measured_comparison;

fn at(seconds: u64) -> String {
    layer2_core::rfc3339_at(std::time::UNIX_EPOCH + std::time::Duration::from_secs(seconds))
}

async fn legacy_fixture(
    store: &ReportingStore,
    status: SandboxStatus,
    from: u64,
) -> (MongoSandboxStore, SandboxUsage) {
    let meter = MongoSandboxStore::open(&store.client, store.db.name())
        .await
        .unwrap();
    let mut parent = crate::store::sandbox_mongo_integration_tests::sandbox("terminal-report");
    parent.owner = SubjectId("owner-1".into());
    parent.name = "Sandbox terminal".into();
    parent.created_at = at(from);
    parent.updated_at = at(from);
    parent.runtime = Some(RuntimeBinding {
        runtime_id: "synthetic-runtime".into(),
        runtime_version: "synthetic-image".into(),
        bound_at: at(from),
        boot_id: None,
        provision_generation: None,
    });
    let mut home = SandboxHome::reserve(
        &parent.id,
        HomeVolumePolicy {
            mode: Default::default(),
            storage_class: "test-home".into(),
            capacity_gib: 10,
            template_name: "test-seed".into(),
            template_generation: "test-generation".into(),
        },
    )
    .unwrap();
    home.bind(HomeVolumeReceipt {
        native_authority: None,
        volume_id: home.request.volume_id.clone(),
        request_sha256: home.request.computed_sha256(),
        storage_reference: "test-home-volume".into(),
        storage_generation: "test-home-generation".into(),
    })
    .unwrap();
    parent.lifecycle.home = Some(Box::new(home));
    meter.create_sandbox(parent.clone()).await.unwrap();
    meter
        .append_sandbox_event(&parent.id, at(from), SandboxEventKind::Created)
        .await
        .unwrap();
    let active = meter.usage(&parent.id).await.unwrap().unwrap();
    let expected = parent.revision;
    parent.status = status;
    parent.failure = (status == SandboxStatus::Failed).then(|| TypedFailure {
        class: "terminal_validation".into(),
        detail: "synthetic refusal".into(),
    });
    parent.retired_at = (status == SandboxStatus::Retired).then(|| at(from + 60));
    if status == SandboxStatus::Retired {
        parent.runtime = None;
        parent.lifecycle.home.as_mut().unwrap().released = true;
    }
    parent.updated_at = at(from + 60);
    parent.revision += 1;
    meter.save_sandbox(expected, parent.clone()).await.unwrap();
    let mut legacy = meter.usage(&parent.id).await.unwrap().unwrap();
    legacy.allocation = Allocation {
        disk_gib: legacy.allocation.disk_gib,
        ..active.allocation
    };
    let mut event = meter
        .usage_events(&parent.id, &PageRequest::default())
        .await
        .unwrap()
        .items
        .into_iter()
        .find(|event| event.revision == legacy.revision)
        .unwrap();
    // Retained v1 state can still carry its preceding compute allocation.
    event.kind = UsageEventKind::StateChanged;
    event.next_allocation = legacy.allocation.clone();
    store
        .collection(sandbox_usage::STATES)
        .replace_one(
            doc! {"_id":parent.id.as_str()},
            sandbox_usage::state_document(&legacy).unwrap(),
        )
        .await
        .unwrap();
    store
        .collection(sandbox_usage::EVENTS)
        .replace_one(
            doc! {"_id":event.transaction_id()},
            sandbox_usage::event_document(&event).unwrap(),
        )
        .await
        .unwrap();
    (meter, legacy)
}

async fn public_report(
    store: &Arc<ReportingStore>,
    from: u64,
    admin: bool,
    collection: bool,
    repaired: bool,
) -> (Value, Vec<Value>) {
    let prefix = if admin {
        "/api/v1/admin/usage/metrics/reports"
    } else {
        "/api/v1/usage/metrics/reports"
    };
    // Both filters select the same fixture rows; a new query avoids report reuse.
    let input =
        json!({"from":from,"to":from+120,"filters":{"search":if repaired {"sandbox"} else {""}}});
    let (status, pending) = request(store, "POST", prefix, "owner-1", admin, input).await;
    assert_eq!(status, 202, "{pending}");
    let id = pending["report_id"].as_str().unwrap();
    let lease = store
        .claim_report("report:0", "terminal-reports")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(lease.report.report_id, id);
    let generated =
        crate::sandbox_metrics::generate_fixture_with_collection(store.clone(), &lease, collection)
            .await
            .unwrap();
    store.finish_report(&lease, generated, true).await.unwrap();
    let path = format!("{prefix}/{id}");
    let (status, report) = request(store, "GET", &path, "owner-1", admin, Value::Null).await;
    assert_eq!(status, 200, "{report}");
    assert_eq!(report["status"], "ready");
    assert_eq!(report["report_complete"], true);
    let (status, rows) = request(
        store,
        "GET",
        &format!("{path}/sandboxes"),
        "owner-1",
        admin,
        Value::Null,
    )
    .await;
    assert_eq!(status, 200, "{rows}");
    (
        report["summary"].clone(),
        rows["items"].as_array().unwrap().clone(),
    )
}

fn assert_report(
    summary: &Value,
    rows: &[Value],
    status: SandboxStatus,
    admin: bool,
    collection: bool,
    repaired: bool,
) {
    let terminal = rows
        .iter()
        .find(|row| row["sandbox_id"] == "terminal-report")
        .unwrap();
    assert_eq!(terminal["status"], status.name());
    let retained_disk = if status == SandboxStatus::Failed {
        10.0
    } else {
        0.0
    };
    assert_eq!(terminal["allocation"]["cpu_cores"].as_f64(), Some(0.0));
    assert_eq!(terminal["allocation"]["memory_bytes"].as_f64(), Some(0.0));
    assert_eq!(
        terminal["allocation"]["disk_bytes"].as_f64(),
        Some(retained_disk * 1_073_741_824.0)
    );
    for (metric, unit) in [("cpu", "cores"), ("memory", "bytes")] {
        assert_eq!(terminal["measured"][metric]["state"], "not_running");
        assert!(terminal["measured"][metric]
            .get(format!("latest_{unit}"))
            .unwrap()
            .is_null());
        assert_eq!(
            terminal["measured"][metric][format!("mean_{unit}")].as_f64(),
            collection.then_some(1.0)
        );
    }
    assert_eq!(terminal["measured"]["disk"]["state"], "unmounted");
    assert_eq!(
        terminal["measured"]["disk"]["last_observed"].as_f64(),
        collection.then_some(1.0)
    );
    let period = &terminal["allocated_period"];
    assert_eq!(period["uptime_seconds"].as_f64(), Some(60.0));
    assert_eq!(period["cpu_core_hours"].as_f64(), Some(1.0 / 60.0));
    assert_eq!(period["memory_gib_hours"].as_f64(), Some(1.0 / 30.0));
    assert_eq!(
        period["disk_gib_hours"].as_f64(),
        Some((10.0 + retained_disk) / 60.0)
    );
    assert_eq!(
        period["projected_tail_seconds"],
        if repaired { 0 } else { 60 }
    );
    assert_eq!(summary["sandbox_count"], if admin { 2 } else { 1 });
    assert_eq!(summary["running_count"], u64::from(admin));
    assert_eq!(
        summary["total_allocated_capacity"]["cpu_cores"].as_f64(),
        Some(if admin { 2.0 } else { 0.0 })
    );
    assert_eq!(
        summary["total_allocated_capacity"]["memory_bytes"].as_f64(),
        Some(if admin { 1_073_741_824.0 } else { 0.0 })
    );
    assert_eq!(
        summary["total_allocated_capacity"]["disk_bytes"].as_f64(),
        Some((retained_disk + if admin { 64.0 } else { 0.0 }) * 1_073_741_824.0)
    );
    assert_eq!(rows.len(), if admin { 2 } else { 1 });
    if admin {
        let active = rows.iter().find(|row| row["status"] == "active").unwrap();
        assert_eq!(active["owner"]["subject"], "owner-2");
        assert_eq!(active["allocation"]["cpu_cores"].as_f64(), Some(2.0));
        assert_eq!(
            active["measured"]["cpu"]["state"],
            if collection { "stale" } else { "unavailable" }
        );
        assert_eq!(
            active["measured"]["cpu"]["mean_cores"].as_f64(),
            collection.then_some(1.0)
        );
    } else {
        assert_eq!(summary["measured"]["cpu"]["state"], "not_running");
    }
}

async fn terminal_reports(status: SandboxStatus) {
    for collection in [false, true] {
        let store = fixture().await;
        let from = (now() - 3600) / 60 * 60;
        let (meter, legacy) = legacy_fixture(&store, status, from).await;
        let active = state(2, from);
        seed(&store, std::slice::from_ref(&active)).await;
        measured_fixture(&store, &legacy, from).await;
        measured_fixture(&store, &active, from).await;
        let history: Vec<Document> = store
            .collection(sandbox_usage::EVENTS)
            .find(doc! {})
            .sort(doc! {"revision":1})
            .await
            .unwrap()
            .try_collect()
            .await
            .unwrap();
        for repaired in [false, true] {
            if repaired {
                meter
                    .checkpoint_usage_one(legacy.sandbox_id.as_str(), &at(from + 120))
                    .await
                    .unwrap();
                let saved = meter.usage(&legacy.sandbox_id).await.unwrap().unwrap();
                assert!(!saved.allocation.running);
                assert_eq!(saved.totals, legacy.totals_at(from + 120).unwrap());
                assert_eq!(saved.source_seq, legacy.source_seq);
                assert_eq!(saved.source_revision, legacy.source_revision);
                let retained: Vec<Document> = store
                    .collection(sandbox_usage::EVENTS)
                    .find(doc! {"revision":{"$lte":legacy.revision as i64}})
                    .sort(doc! {"revision":1})
                    .await
                    .unwrap()
                    .try_collect()
                    .await
                    .unwrap();
                assert_eq!(retained, history);
            }
            for admin in [false, true] {
                let (summary, rows) =
                    public_report(&store, from, admin, collection, repaired).await;
                assert_report(&summary, &rows, status, admin, collection, repaired);
            }
        }
        store.db.drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn legacy_failed_reports_agree_before_and_after_checkpoint_repair() {
    terminal_reports(SandboxStatus::Failed).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn legacy_retired_reports_agree_before_and_after_checkpoint_repair() {
    terminal_reports(SandboxStatus::Retired).await;
}
