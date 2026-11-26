use super::super::read_models::measured_usage::{collected, GIB, PRIMARY, SECONDARY};
use super::*;
use layer2_core::sandbox_metrics::aggregate::totals;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn measured_intervals_reconcile_with_owner_reports_and_allocated_usage() {
    let (store, range) = collected().await;
    assert_collection(&store, range).await;

    let (_, owner_path, rows) = report(&store, range, "owner-1", false).await;
    assert_eq!(rows.len(), 1);
    assert_primary(&rows[0]);
    let (status, own_detail) = request(
        &store,
        "GET",
        &format!("{owner_path}/sandboxes/{PRIMARY}"),
        "owner-1",
        false,
        Value::Null,
    )
    .await;
    assert_eq!(status, 200, "{own_detail}");
    assert_primary(&own_detail);
    let (status, _) = request(&store, "GET", &owner_path, "owner-2", false, Value::Null).await;
    assert_eq!(status, 404);
    let (status, _) = request(
        &store,
        "GET",
        &format!("{owner_path}/sandboxes/{SECONDARY}"),
        "owner-1",
        false,
        Value::Null,
    )
    .await;
    assert_eq!(status, 404);

    let (_, _, rows) = report(&store, range, "owner-2", false).await;
    assert_eq!(rows.len(), 1);
    assert_secondary(&rows[0]);

    let (summary, _, rows) = report(&store, range, "operator", true).await;
    assert_eq!(rows.len(), 2);
    assert_eq!(summary["sandbox_count"], 2);
    assert_primary(
        rows.iter()
            .find(|row| row["sandbox_id"] == PRIMARY)
            .unwrap(),
    );
    assert_secondary(
        rows.iter()
            .find(|row| row["sandbox_id"] == SECONDARY)
            .unwrap(),
    );
    assert_number(&summary["allocated_period"]["uptime_seconds"], 180. + 300.);
    assert_number(
        &summary["allocated_period"]["disk_gib_hours"],
        (10. * 300. + 10. * 300.) / 3600.,
    );
    store.db.drop().await.unwrap();
}

async fn assert_collection(store: &ReportingStore, range: Range) {
    let bindings = store.bindings(&[PRIMARY.into()], range).await.unwrap();
    let home: Vec<_> = bindings
        .iter()
        .filter(|row| row.kind == BindingKind::Home)
        .collect();
    assert_eq!(home.len(), 1);
    assert_eq!(home[0].range(range.to), range);
    let mut compute: Vec<_> = bindings
        .iter()
        .filter(|row| row.kind == BindingKind::Compute)
        .collect();
    compute.sort_by_key(|row| row.valid_from);
    assert_eq!(compute.len(), 2);
    assert_eq!(compute[0].runtime_generation, Some(1));
    assert_eq!(
        compute[0].range(range.to),
        Range {
            from: range.from,
            to: range.from + 90
        }
    );
    assert_eq!(compute[1].runtime_generation, Some(2));
    assert_eq!(
        compute[1].range(range.to),
        Range {
            from: range.from + 120,
            to: range.from + 180
        }
    );
    assert_eq!(compute[0].pod_name, compute[1].pod_name);
    assert_ne!(compute[0].pod_uid, compute[1].pod_uid);

    let mut session = store.snapshot().await.unwrap();
    let rollups = store
        .metric_rollups(&mut session, &[PRIMARY.into(), SECONDARY.into()], range)
        .await
        .unwrap();
    assert_eq!(rollups.len(), 2);
    let rollup = rollups
        .iter()
        .find(|row| row.sandbox_id == PRIMARY)
        .unwrap();
    assert_eq!(rollup.owner_subject, "owner-1");
    assert!(rollup.source_complete);
    let minutes = &rollup.minutes;
    assert_eq!(minutes.len(), 5);
    // Two deltas in generation 1, then a fresh baseline at offset 120
    // and one delta in generation 2. The replacement's first value is high.
    for (index, expected) in [(75., 60.), (0., 0.), (15., 30.), (0., 0.), (0., 0.)]
        .into_iter()
        .enumerate()
    {
        assert_eq!(minutes[index].from, range.from + index as u64 * 60);
        let cpu = &minutes[index].metrics[Metric::Cpu.index()];
        assert_eq!((cpu.integral, cpu.covered_seconds), expected);
    }
    let [cpu, memory, disk] = totals(minutes);
    assert_eq!(
        (cpu.integral, cpu.covered_seconds, cpu.expected_seconds),
        (75. + 15., 90., 150.)
    );
    let gib = GIB as f64;
    assert_eq!(
        memory.integral,
        (60. + 2. * 30. + 3. * 30. + 2. * 30.) * gib
    );
    assert_eq!(
        (memory.covered_seconds, memory.expected_seconds),
        (150., 150.)
    );
    // One physical PVC contributes through pause. Its sample at offset 60
    // expires at 150, leaving half of the third minute uncovered.
    assert_eq!(
        minutes[2].metrics[Metric::Disk.index()].covered_seconds,
        30.
    );
    assert_eq!(
        disk.integral,
        (2. * 60. + 3. * 90. + 4. * 60. + 5. * 60.) * gib
    );
    assert_eq!((disk.covered_seconds, disk.expected_seconds), (270., 300.));
    let secondary = rollups
        .iter()
        .find(|row| row.sandbox_id == SECONDARY)
        .unwrap();
    assert_eq!(secondary.owner_subject, "owner-2");
    assert_eq!(
        totals(&secondary.minutes)[Metric::Disk.index()].integral,
        8. * 300. * gib
    );
}

async fn report(
    store: &Arc<ReportingStore>,
    range: Range,
    owner: &str,
    admin: bool,
) -> (Value, String, Vec<Value>) {
    let prefix = if admin {
        "/api/v1/admin/usage/metrics/reports"
    } else {
        "/api/v1/usage/metrics/reports"
    };
    let (status, pending) = request(
        store,
        "POST",
        prefix,
        owner,
        admin,
        json!({"from": range.from, "to": range.to, "step_seconds": 60}),
    )
    .await;
    assert_eq!(status, 202, "{pending}");
    let id = pending["report_id"].as_str().unwrap();
    let lease = store
        .claim_report("report:0", "measured-comparison")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(lease.report.report_id, id);
    let generated = crate::sandbox_metrics::generate_fixture(store.clone(), &lease)
        .await
        .unwrap();
    store.finish_report(&lease, generated, true).await.unwrap();
    let path = format!("{prefix}/{id}");
    let (status, ready) = request(store, "GET", &path, owner, admin, Value::Null).await;
    assert_eq!(status, 200, "{ready}");
    assert_eq!(ready["status"], "ready");
    assert_eq!(ready["report_complete"], true);
    let (status, page) = request(
        store,
        "GET",
        &format!("{path}/sandboxes"),
        owner,
        admin,
        Value::Null,
    )
    .await;
    assert_eq!(status, 200, "{page}");
    (
        ready["summary"].clone(),
        path,
        page["items"].as_array().unwrap().clone(),
    )
}

fn assert_primary(row: &Value) {
    assert_eq!(row["sandbox_id"], PRIMARY);
    assert_eq!(row["owner"]["subject"], "owner-1");
    assert_eq!(row["agent_count"], 2);
    assert_eq!(row["status"], "paused");
    let allocated = &row["allocated_period"];
    assert_eq!(allocated["complete"], true);
    assert_number(&allocated["uptime_seconds"], 180.);
    assert_number(&allocated["cpu_core_hours"], 180. / 3600.);
    assert_number(&allocated["memory_gib_hours"], 2. * 180. / 3600.);
    assert_number(&allocated["disk_gib_hours"], 10. * 300. / 3600.);
    let measured = &row["measured"];
    assert_eq!(measured["cpu"]["state"], "not_running");
    assert_eq!(measured["memory"]["state"], "not_running");
    assert_eq!(measured["disk"]["state"], "unmounted");
    assert_number(&measured["cpu"]["observed_core_hours"], 90. / 3600.);
    assert_number(&measured["memory"]["observed_gib_hours"], 270. / 3600.);
    assert_number(&measured["disk"]["observed_gib_hours"], 930. / 3600.);
    assert_number(&measured["disk"]["last_observed"], (5 * GIB) as f64);
    for (metric, covered, expected, unmapped) in [
        ("cpu", 90., 180., 30.),
        ("memory", 150., 180., 30.),
        ("disk", 270., 300., 0.),
    ] {
        assert_number(&measured[metric]["covered_seconds"], covered);
        assert_number(&measured[metric]["expected_seconds"], expected);
        assert_number(&measured[metric]["unknown_mapping_seconds"], unmapped);
        assert_number(&measured[metric]["coverage_ratio"], covered / expected);
    }
    assert_eq!(
        row["billing"],
        json!({"basis":"allocated","status":"not_integrated","charges":null})
    );
}

fn assert_secondary(row: &Value) {
    assert_eq!(row["sandbox_id"], SECONDARY);
    assert_eq!(row["owner"]["subject"], "owner-2");
    assert_eq!(row["agent_count"], 0);
    assert_number(
        &row["measured"]["disk"]["observed_gib_hours"],
        8. * 300. / 3600.,
    );
    assert_number(&row["measured"]["disk"]["coverage_ratio"], 1.);
    assert_number(&row["allocated_period"]["uptime_seconds"], 300.);
}

fn assert_number(actual: &Value, expected: f64) {
    let actual = actual
        .as_f64()
        .expect("the report must contain a measured or allocated quantity");
    assert!(
        (actual - expected).abs() < 1e-9,
        "expected {expected}, got {actual}"
    );
}
