use super::{allocated::request, *};
const BASE: &str = "/api/v1/admin/usage/metrics/reports";
const ADMIN: &str = "operator";

async fn get(store: &Arc<ReportingStore>, path: &str) -> (u16, serde_json::Value) {
    request(store, "GET", path, ADMIN, true, serde_json::Value::Null).await
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn admin_filter_change_and_cached_pages_survive_one_slow_successful_checkout() {
    let store = fixture().await;
    let at = now();
    let from = at - 86_400;
    let states: Vec<_> = (0..400)
        .map(|n| {
            let mut s = state(n, from);
            if n >= 40 {
                s.status = "paused".into();
                s.allocation.running = false;
            }
            s
        })
        .collect();
    seed(&store, &states).await;
    let input = serde_json::json!({"from":from,"to":at});
    let (status, full) = request(&store, "POST", BASE, ADMIN, true, input.clone()).await;
    assert_eq!(status, 202);
    let lease = store
        .claim_report("report:0", "full")
        .await
        .unwrap()
        .unwrap();
    let generated =
        crate::sandbox_metrics::generate_fixture_with_collection(store.clone(), &lease, false)
            .await
            .unwrap();
    let summary = generated.summary.as_ref().unwrap();
    assert_eq!(summary.sandbox_count, 400);
    assert_eq!(summary.running_count, 40);
    assert!(summary.allocated_period.complete);
    store.finish_report(&lease, generated, true).await.unwrap();
    let full_id = full["report_id"].as_str().unwrap();
    let previous_page = format!("{BASE}/{full_id}/sandboxes");
    assert_eq!(get(&store, &previous_page).await.0, 200);

    let mut active_input = input;
    active_input["filters"] = serde_json::json!({"statuses":["active"]});
    let (status, active) = request(&store, "POST", BASE, ADMIN, true, active_input.clone()).await;
    assert_eq!(status, 202);
    let active_id = active["report_id"].as_str().unwrap();
    let active_url = format!("{BASE}/{active_id}");
    for _ in 0..3 {
        let (status, pending) = get(&store, &active_url).await;
        assert_eq!(status, 200);
        assert_eq!(pending["status"], "pending");
        assert_eq!(get(&store, &previous_page).await.0, 200);
    }
    let lease = store
        .claim_report("report:0", "active")
        .await
        .unwrap()
        .unwrap();

    // Fill all four pool slots with bounded commands, then queue exactly one
    // successful report checkout for >250 ms. A separate client controls the
    // failpoint so synchronization does not itself wait for a reporting slot.
    let control = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap()
        .database("admin");
    let configured = control
        .run_command(doc! {
            "configureFailPoint":"failCommand", "mode":{"times":4},
            "data":{"failCommands":["ping"],"blockConnection":true,"blockTimeMS":700_i64}
        })
        .await
        .unwrap();
    let entered = configured
        .get_i64("count")
        .or_else(|_| configured.get_i32("count").map(i64::from))
        .unwrap();
    let mut blockers = Vec::new();
    for _ in 0..4 {
        let db = store.db.clone();
        blockers.push(tokio::spawn(async move {
            db.run_command(doc! {"ping":1}).await
        }));
    }
    control
        .run_command(doc! {
            "waitForFailPoint":"failCommand", "timesEntered":entered+4, "maxTimeMS":2000_i64
        })
        .await
        .unwrap();
    let started = std::time::Instant::now();
    let generated =
        crate::sandbox_metrics::generate_fixture_with_collection(store.clone(), &lease, false)
            .await;
    for blocker in blockers {
        blocker.await.unwrap().unwrap();
    }
    control
        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
        .await
        .unwrap();
    assert!(started.elapsed() > Duration::from_millis(250));
    // Finalize exactly as the worker does, including a failed generation.
    // #1386 produces the production pair: new poll 503 and cached page 429.
    let generation_error = generated.as_ref().err().map(ToString::to_string);
    store
        .finish_report(
            &lease,
            generated
                .as_ref()
                .map(Clone::clone)
                .unwrap_or_else(|_| lease.report.clone()),
            generated.is_ok(),
        )
        .await
        .unwrap();
    assert_eq!(
        (
            get(&store, &active_url).await.0,
            get(&store, &previous_page).await.0
        ),
        (200, 200),
        "generation error: {generation_error:?}"
    );
    let generated = generated.unwrap();
    assert_eq!(generated.summary.as_ref().unwrap().sandbox_count, 40);
    assert!(
        generated
            .summary
            .as_ref()
            .unwrap()
            .allocated_period
            .complete
    );
    assert_eq!(
        generated
            .summary
            .as_ref()
            .unwrap()
            .measured
            .as_ref()
            .unwrap()
            .cpu
            .mean,
        None
    );
    for _ in 0..3 {
        let (status, report) = get(&store, &active_url).await;
        assert_eq!(status, 200);
        assert_eq!(report["status"], "ready");
        let (status, rows) = get(&store, &format!("{active_url}/sandboxes")).await;
        assert_eq!(status, 200);
        assert_eq!(rows["total_count"], 40);
        assert_eq!(rows["items"].as_array().unwrap().len(), 40);
        let (status, previous) = get(&store, &previous_page).await;
        assert_eq!(status, 200);
        assert_eq!(previous["total_count"], 400);
    }
    assert!(
        store
            .collection(JOBS)
            .find_one(doc! {"_id":active_id})
            .await
            .unwrap()
            .unwrap()
            .get_i64("reserved_bytes")
            .unwrap()
            < store.budgets.report_bytes
    );
    // A genuine circuit still refuses work/pages; ownership and limits remain.
    store
        .circuit
        .store(now() + 30, std::sync::atomic::Ordering::Relaxed);
    let (status, failure) = get(&store, &previous_page).await;
    assert_eq!(status, 429);
    assert_eq!(failure["code"], "reporting_budget_exceeded");
    assert_eq!(
        request(&store, "POST", BASE, ADMIN, true, active_input)
            .await
            .0,
        429
    );
    store.circuit.store(0, std::sync::atomic::Ordering::Relaxed);
    store.db.drop().await.unwrap();
}
