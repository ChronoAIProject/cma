use super::{allocated::request, *};

const BASE: &str = "/api/v1/admin/usage/metrics/reports";

fn input(from: u64) -> serde_json::Value {
    serde_json::json!({"from":from,"to":from+60})
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn cold_reports_wake_workers_and_complete_in_first_response() {
    let subscriber = tracing_subscriber::fmt()
        .with_test_writer()
        .with_max_level(tracing::Level::WARN)
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    let store = fixture().await;
    let from = (now() - 3600) / 60 * 60;
    seed(
        &store,
        &(0..24).map(|id| state(id, from)).collect::<Vec<_>>(),
    )
    .await;
    let worker = tokio::spawn(crate::sandbox_metrics::reports_fixture(store.clone()));

    // Two different queries must both wake the actual worker, including while
    // its two-second recovery timer is idle after the first completed job.
    for offset in [0, 60] {
        let started = std::time::Instant::now();
        let query = input(from + offset);
        let (first, coalesced) = tokio::join!(
            request(&store, "POST", BASE, "operator", true, query.clone()),
            request(&store, "POST", BASE, "operator", true, query.clone()),
        );
        assert_eq!(first.0, 200, "{:?}", first.1);
        assert_eq!(coalesced.0, 200, "{:?}", coalesced.1);
        assert_eq!(first.1["report_id"], coalesced.1["report_id"]);
        assert_eq!(first.1["status"], "ready");
        assert_eq!(first.1["summary"]["sandbox_count"], 24);
        let id = first.1["report_id"].as_str().unwrap();
        let rows_path = format!("{BASE}/{id}/sandboxes?limit=50");
        let series_path = format!("{BASE}/{id}/series?metric=cpu");
        let (rows, series) = tokio::join!(
            request(
                &store,
                "GET",
                &rows_path,
                "operator",
                true,
                serde_json::Value::Null
            ),
            request(
                &store,
                "GET",
                &series_path,
                "operator",
                true,
                serde_json::Value::Null
            ),
        );
        assert_eq!(rows.0, 200);
        assert_eq!(rows.1["items"].as_array().unwrap().len(), 24);
        assert_eq!(series.0, 200);
        let elapsed = started.elapsed();
        eprintln!("local admin report + 24 rows + CPU series: {elapsed:?}");
        assert!(elapsed < Duration::from_secs(2), "{elapsed:?}");
        let cached = request(&store, "POST", BASE, "operator", true, query).await;
        assert_eq!(cached.0, 200);
        assert_eq!(cached.1["report_id"], first.1["report_id"]);
        assert_eq!(
            request(
                &store,
                "GET",
                &format!("{BASE}/{id}"),
                "foreign",
                true,
                serde_json::Value::Null
            )
            .await
            .0,
            404
        );
    }
    worker.abort();
    assert!(worker.await.unwrap_err().is_cancelled());
    store.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn pending_submission_has_a_bounded_wait_and_keeps_its_job() {
    let store = fixture().await;
    // A subscribed but occupied worker cannot extend the first response budget.
    let mut work = store.report_work();
    let from = (now() - 3600) / 60 * 60;
    let started = std::time::Instant::now();
    let (status, pending) = request(&store, "POST", BASE, "operator", true, input(from)).await;
    assert_eq!(status, 202);
    assert_eq!(pending["status"], "pending");
    assert_eq!(pending["retry_after_seconds"], 2);
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(work.has_changed().unwrap());
    work.borrow_and_update();
    let lease = store
        .claim_report("report:0", "later")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        lease.report.report_id,
        pending["report_id"].as_str().unwrap()
    );
    let generated =
        crate::sandbox_metrics::generate_fixture_with_collection(store.clone(), &lease, false)
            .await
            .unwrap();
    store.finish_report(&lease, generated, true).await.unwrap();
    let (status, ready) = request(&store, "POST", BASE, "operator", true, input(from)).await;
    assert_eq!(status, 200);
    assert_eq!(ready["report_id"], pending["report_id"]);
    assert!(
        !work.has_changed().unwrap(),
        "cache hits must not wake workers"
    );
    store.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn first_response_propagates_generation_failure() {
    let store = fixture().await;
    let from = (now() - 3600) / 60 * 60;
    seed(&store, &[state(1, from)]).await;
    store
        .collection(super::super::super::sandbox_usage::STATES)
        .update_many(doc! {}, doc! {"$set":{"record":{}}})
        .await
        .unwrap();
    let worker = tokio::spawn(crate::sandbox_metrics::reports_fixture(store.clone()));
    let (status, body) = request(&store, "POST", BASE, "operator", true, input(from)).await;
    assert_eq!(status, 503);
    assert_eq!(body["code"], "usage_unavailable");
    worker.abort();
    assert!(worker.await.unwrap_err().is_cancelled());
    store.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn first_response_reauthorizes_ownership_after_completion() {
    let store = fixture().await;
    let from = (now() - 3600) / 60 * 60;
    let own = state(1, from);
    seed(&store, std::slice::from_ref(&own)).await;
    let mut work = store.report_work();
    let worker_store = store.clone();
    let worker = tokio::spawn(async move {
        work.changed().await.unwrap();
        let lease = worker_store
            .claim_report("report:0", "transfer")
            .await
            .unwrap()
            .unwrap();
        let generated = crate::sandbox_metrics::generate_fixture_with_collection(
            worker_store.clone(),
            &lease,
            false,
        )
        .await
        .unwrap();
        worker_store
            .collection(super::super::super::sandbox_usage::STATES)
            .update_one(
                doc! {"_id":own.sandbox_id.as_str()},
                doc! {"$set":{"owner":"new-owner"}},
            )
            .await
            .unwrap();
        worker_store
            .finish_report(&lease, generated, true)
            .await
            .unwrap();
    });
    let (status, _) = request(
        &store,
        "POST",
        "/api/v1/usage/metrics/reports",
        "owner-1",
        false,
        input(from),
    )
    .await;
    assert_eq!(status, 404);
    worker.await.unwrap();
    store.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn first_response_observes_completion_on_another_replica() {
    let store = fixture().await;
    let _local_worker = store.report_work();
    let remote = ReportingStore::connect(
        &std::env::var("CHRONO_TEST_MONGODB_URI").unwrap(),
        store.db.name(),
        &[19; 32],
    )
    .await
    .unwrap();
    let mut work = store.report_work();
    let worker = tokio::spawn(async move {
        work.changed().await.unwrap();
        let lease = remote
            .claim_report("report:0", "remote")
            .await
            .unwrap()
            .unwrap();
        let generated =
            crate::sandbox_metrics::generate_fixture_with_collection(remote.clone(), &lease, false)
                .await
                .unwrap();
        // This store has separate process-local notifications.
        remote.finish_report(&lease, generated, true).await.unwrap();
    });
    let from = (now() - 3600) / 60 * 60;
    let (status, ready) = request(&store, "POST", BASE, "operator", true, input(from)).await;
    assert_eq!(status, 200, "{ready:?}");
    assert_eq!(ready["status"], "ready");
    worker.await.unwrap();
    store.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn default_day_report_serves_four_hundred_sandboxes_without_polling() {
    let store = fixture().await;
    let from = (now() - 86_400) / 60 * 60;
    let states: Vec<_> = (0..400).map(|id| state(id, from)).collect();
    for batch in states.chunks(200) {
        seed(&store, batch).await;
    }
    let worker = tokio::spawn(crate::sandbox_metrics::reports_fixture(store.clone()));
    let started = std::time::Instant::now();
    let (status, report) = request(
        &store,
        "POST",
        BASE,
        "operator",
        true,
        serde_json::json!({"from":from,"to":from+86_400}),
    )
    .await;
    assert_eq!(status, 200, "{report:?}");
    assert_eq!(report["summary"]["sandbox_count"], 400);
    let id = report["report_id"].as_str().unwrap();
    let (status, rows) = request(
        &store,
        "GET",
        &format!("{BASE}/{id}/sandboxes?limit=50"),
        "operator",
        true,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(rows["items"].as_array().unwrap().len(), 50);
    assert_eq!(rows["total_count"], 400);
    assert!(rows["next"].is_string());
    let elapsed = started.elapsed();
    eprintln!("local 24-hour admin report, 400 sandboxes, first 50 rows: {elapsed:?}");
    assert!(elapsed < Duration::from_secs(2), "{elapsed:?}");
    worker.abort();
    assert!(worker.await.unwrap_err().is_cancelled());
    store.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn worker_drains_jobs_admitted_before_it_subscribed() {
    let store = fixture().await;
    let at = now();
    let from = (at - 3600) / 60 * 60;
    for (offset, principal) in [(0, "operator-a"), (60, "operator-b"), (120, "operator-c")] {
        let query: ReportInput = serde_json::from_value(input(from + offset)).unwrap();
        store
            .admit(scope(principal, true), query.validate(at).unwrap(), at)
            .await
            .unwrap();
    }
    let worker = tokio::spawn(crate::sandbox_metrics::reports_fixture(store.clone()));
    let (a, b, c) = tokio::join!(
        request(&store, "POST", BASE, "operator-a", true, input(from)),
        request(&store, "POST", BASE, "operator-b", true, input(from + 60)),
        request(&store, "POST", BASE, "operator-c", true, input(from + 120)),
    );
    for response in [a, b, c] {
        assert_eq!(response.0, 200, "{:?}", response.1);
        assert_eq!(response.1["status"], "ready");
    }
    worker.abort();
    assert!(worker.await.unwrap_err().is_cancelled());
    store.db.drop().await.unwrap();
}
