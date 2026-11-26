use super::*;
use axum::{
    body::{to_bytes, Body},
    http::Request,
};
use layer2_core::{Authorization, Capability};
use tower::ServiceExt;

pub(super) async fn request(
    store: &Arc<ReportingStore>,
    method: &str,
    path: &str,
    owner: &str,
    admin: bool,
    body: serde_json::Value,
) -> (u16, serde_json::Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    request
        .extensions_mut()
        .insert(layer1_public::AuthenticatedSubject(SubjectId(owner.into())));
    request
        .extensions_mut()
        .insert(Authorization::new([if admin {
            Capability::ReadAllSandboxes
        } else {
            Capability::UseOwnSandboxes
        }]));
    let response = layer1_public::rest::sandbox_metrics_router(MetricsService::new(store.clone()))
        .oneshot(request)
        .await
        .unwrap();
    let status = response.status().as_u16();
    let body = to_bytes(response.into_body(), 1_000_000).await.unwrap();
    (status, serde_json::from_slice(&body).unwrap())
}
pub(super) async fn measured_fixture(store: &ReportingStore, state: &SandboxUsage, from: u64) {
    let holder = uuid::Uuid::new_v4().to_string();
    assert!(store.lease("rollups", &holder, 70).await.unwrap());
    let a = Aggregate {
        integral: 60.,
        display_integral: 60.,
        covered_seconds: 60.,
        expected_seconds: 60.,
        observed_at: Some((from + 30) as f64),
        latest: Some(1.),
        available_from: Some(from as f64),
        ..Default::default()
    };
    store
        .save_metric_rollups(
            "rollups",
            &holder,
            &[MetricRollup {
                schema_version: 1,
                algorithm_version: ALGORITHM_VERSION,
                source: SOURCE.into(),
                sandbox_id: state.sandbox_id.to_string(),
                owner_subject: state.owner.to_string(),
                day: from / 86_400 * 86_400,
                chunk: ((from % 86_400) / 3600) as u16,
                mapping_revision: 0,
                cutoff: from + 60,
                source_complete: true,
                minutes: vec![Minute {
                    from,
                    to: from + 60,
                    metrics: [a.clone(), a.clone(), a.clone()],
                }],
            }],
        )
        .await
        .unwrap();
    store
        .save_live(
            &holder,
            &[(
                state.sandbox_id.to_string(),
                super::super::super::sandbox_metric_rollups::LiveReading {
                    owner: state.owner.to_string(),
                    observed_at: now(),
                    metrics: [a.clone(), a.clone(), a],
                },
            )],
        )
        .await
        .unwrap();
    store.release("rollups", &holder).await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn allocated_reports_serve_both_scopes_without_measured_collection() {
    let store = fixture().await;
    let from = (now() - 3600) / 60 * 60;
    let own = state(1, from);
    seed(&store, &[own.clone(), state(2, from)]).await;
    measured_fixture(&store, &own, from).await;
    // Retained measurements must not leak into a collection-off report.
    for (admin, count) in [(false, 1), (true, 2)] {
        let prefix = if admin {
            "/api/v1/admin/usage/metrics/reports"
        } else {
            "/api/v1/usage/metrics/reports"
        };
        let input = serde_json::json!({"from":from,"to":from+60});
        let (status, pending) = request(&store, "POST", prefix, "owner-1", admin, input).await;
        assert_eq!(status, 202);
        let id = pending["report_id"].as_str().unwrap();
        let lease = store
            .claim_report("report:0", "allocated")
            .await
            .unwrap()
            .unwrap();
        let generated =
            crate::sandbox_metrics::generate_fixture_with_collection(store.clone(), &lease, false)
                .await
                .unwrap();
        let summary = generated.summary.as_ref().unwrap();
        assert_eq!(summary.sandbox_count, count);
        assert_eq!(summary.running_count, count);
        assert!(summary.allocated_period.complete);
        assert_eq!(summary.allocated_period.uptime_seconds, count as f64 * 60.);
        assert_eq!(summary.allocated_period.cpu_core_hours, count as f64 / 30.);
        assert_eq!(
            summary.total_allocated_capacity.cpu_cores,
            count as f64 * 2.
        );
        assert!(summary.infrastructure.is_none());
        assert_eq!(
            summary.measured.as_ref().unwrap().cpu.state,
            MetricState::Unavailable
        );
        assert_eq!(summary.measured.as_ref().unwrap().cpu.mean, None);
        assert_eq!(generated.available_from, None);
        assert_eq!(generated.source_health, "source_unavailable");
        store.finish_report(&lease, generated, true).await.unwrap();
        let (status, ready) = request(
            &store,
            "GET",
            &format!("{prefix}/{id}"),
            "owner-1",
            admin,
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(status, 200);
        assert_eq!(ready["status"], "ready");
        assert_eq!(ready["report_complete"], true);
        let (status, rows) = request(
            &store,
            "GET",
            &format!("{prefix}/{id}/sandboxes"),
            "owner-1",
            admin,
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(status, 200);
        assert_eq!(rows["items"].as_array().unwrap().len(), count as usize);
        for row in rows["items"].as_array().unwrap() {
            assert_eq!(row["measured"]["cpu"]["state"], "unavailable");
            assert!(row["measured"]["cpu"]["mean"].is_null());
        }
        assert_eq!(
            request(
                &store,
                "GET",
                &format!("{prefix}/{id}"),
                "another",
                admin,
                serde_json::Value::Null
            )
            .await
            .0,
            404
        );
        if admin {
            assert_eq!(
                request(
                    &store,
                    "GET",
                    &format!("{prefix}/{id}"),
                    "owner-1",
                    false,
                    serde_json::Value::Null
                )
                .await
                .0,
                403
            );
        }
    }
    store.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn collection_enabled_keeps_measured_results_and_corrupt_source_failures() {
    let store = fixture().await;
    let from = (now() - 3600) / 60 * 60;
    let own = state(1, from);
    seed(&store, std::slice::from_ref(&own)).await;
    measured_fixture(&store, &own, from).await;
    let query = ReportInput {
        from: Some(from),
        to: Some(from + 60),
        ..Default::default()
    }
    .validate(now())
    .unwrap();
    let report = store
        .submit(scope("owner-1", false), query, now())
        .await
        .unwrap();
    let lease = store
        .claim_report("report:0", "measured")
        .await
        .unwrap()
        .unwrap();
    let generated = crate::sandbox_metrics::generate_fixture(store.clone(), &lease)
        .await
        .unwrap();
    assert_eq!(generated.source_health, "ok");
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
        Some(1.)
    );
    let collection = super::super::super::sandbox_metric_rollups::COLLECTION;
    store
        .collection(collection)
        .update_many(doc! {}, doc! {"$set":{"record":{}}})
        .await
        .unwrap();
    assert!(
        crate::sandbox_metrics::generate_fixture(store.clone(), &lease)
            .await
            .is_err()
    );
    store
        .finish_report(&lease, report.clone(), false)
        .await
        .unwrap();
    let (status, error) = request(
        &store,
        "GET",
        &report.poll_url,
        "owner-1",
        false,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, 503);
    assert_eq!(error["code"], "usage_unavailable");
    // A fresh report has its own fenced cache; a generation is never appended
    // twice to the same lease (takeover would clear its previous rows).
    let query = report.query.clone();
    store
        .submit(scope("owner-1", true), query.clone(), now())
        .await
        .unwrap();
    let off = store
        .claim_report("report:0", "allocated")
        .await
        .unwrap()
        .unwrap();
    let generated =
        crate::sandbox_metrics::generate_fixture_with_collection(store.clone(), &off, false)
            .await
            .unwrap();
    store.finish_report(&off, generated, true).await.unwrap();
    let mut query = query;
    query.filters.search = "sandbox".into();
    store
        .submit(scope("owner-1", false), query, now())
        .await
        .unwrap();
    let corrupt = store
        .claim_report("report:0", "corrupt-allocation")
        .await
        .unwrap()
        .unwrap();
    // Allocation corruption must still fail, even with collection off.
    store
        .collection(super::super::super::sandbox_usage::STATES)
        .update_one(
            doc! {"_id":own.sandbox_id.as_str()},
            doc! {"$set":{"record":{}}},
        )
        .await
        .unwrap();
    assert!(crate::sandbox_metrics::generate_fixture_with_collection(
        store.clone(),
        &corrupt,
        false
    )
    .await
    .is_err());
    store.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn terminal_reports_release_unused_reservations_without_undercharging_cache() {
    let store = fixture().await;
    let at = now();
    let query = ReportInput::default().validate(at).unwrap();
    for n in 0..12 {
        // The same owner refreshes both scopes across minute boundaries.
        let report = store
            .submit(scope("owner", n % 2 == 0), query.clone(), at + n * 60)
            .await
            .unwrap();
        let lease = store
            .claim_report("report:0", "refresh")
            .await
            .unwrap()
            .unwrap();
        let chart = doc! {"_id":format!("chart-{n}"),"report_id":&report.report_id,"document_schema_version":1_i64,"record":"cache"};
        let bytes = mongodb::bson::serialize_to_vec(&chart).unwrap().len() as i64;
        store
            .write_report_chunk(&lease, &[], vec![chart])
            .await
            .unwrap();
        store
            .finish_report(&lease, report.clone(), n % 3 != 0)
            .await
            .unwrap();
        let saved = store
            .collection(JOBS)
            .find_one(doc! {"_id":&report.report_id})
            .await
            .unwrap()
            .unwrap();
        assert_eq!(saved.get_i64("reserved_bytes").unwrap(), bytes);
        assert_eq!(saved.get_i64("bytes").unwrap(), bytes);
        assert!(saved.get_bool("reserved").unwrap());
        let retained: Report = decode(&saved).unwrap();
        assert_eq!(retained.schema_version, 1);
        assert_eq!(
            retained.status,
            if n % 3 == 0 {
                ReportStatus::Failed
            } else {
                ReportStatus::Ready
            }
        );
        // A stale worker must not write after releasing its work reservation.
        assert!(store.write_report_chunk(&lease, &[], vec![]).await.is_err());
    }
    assert_eq!(
        store
            .collection(CHARTS)
            .count_documents(doc! {})
            .await
            .unwrap(),
        12
    );
    let mut tight = (*store).clone();
    tight.budgets.tenant_bytes = tight.budgets.report_bytes;
    assert!(matches!(
        tight.submit(scope("owner", true), query, at + 720).await,
        Err(MetricsError::Busy)
    ));
    store.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn allocated_report_tolerates_multiple_bounded_database_reads() {
    let store = fixture().await;
    let from = (now() - 3600) / 60 * 60;
    seed(&store, &[state(1, from)]).await;
    let query = ReportInput {
        from: Some(from),
        to: Some(from + 60),
        ..Default::default()
    }
    .validate(now())
    .unwrap();
    store
        .submit(scope("owner-1", false), query, now())
        .await
        .unwrap();
    let lease = store
        .claim_report("report:0", "bounded-reads")
        .await
        .unwrap()
        .unwrap();
    // Three inventory commands each finish below the 2s DB deadline, while
    // their combined wall time exceeds it. This used to poison the next read.
    store.client.database("admin").run_command(doc! {
        "configureFailPoint":"failCommand", "mode":{"times":3},
        "data":{"failCommands":["find","aggregate"],"blockConnection":true,"blockTimeMS":750_i64}
    }).await.unwrap();
    let generated =
        crate::sandbox_metrics::generate_fixture_with_collection(store.clone(), &lease, false)
            .await;
    store
        .client
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
        .await
        .unwrap();
    let generated = generated.unwrap();
    assert!(
        generated
            .summary
            .as_ref()
            .unwrap()
            .allocated_period
            .complete
    );
    store.check_circuit(now()).unwrap();
    store.finish_report(&lease, generated, true).await.unwrap();
    store.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn personal_usage_matches_displayed_sandbox_prefixes() {
    let store = fixture().await;
    let from = (now() - 3600) / 60 * 60;
    let mut own = state(1, from);
    own.sandbox_id = SandboxId(format!("sbx_{:032x}", u128::MAX));
    own.name = "  Research Workshop".into();
    let mut control = state(13, from);
    control.name = "Other Sandbox".into();
    let mut foreign = state(2, from);
    foreign.name = own.name.clone();
    seed(&store, &[own.clone(), control, foreign]).await;

    let path = "/api/v1/usage/metrics/reports";
    for (search, expected) in [
        ("  ReSEARch ", true),
        ("sbx_ffff", true),
        ("fffffff", true),
        ("Research Workshop", true),
        ("sbx_ffffffffffffffffffffffffffffffff", true),
        ("ffffffffffffffffffffffffffffffff", true),
        ("sbx_ffffffffff…ffffffff", true),
        ("sbx_ffffffffff…fffffffe", false),
        ("Workshop", false),
    ] {
        let (status, pending) = request(
            &store,
            "POST",
            path,
            "owner-1",
            false,
            serde_json::json!({"from":from,"to":from+60,"filters":{"search":search}}),
        )
        .await;
        assert_eq!(status, 202, "{search}: {pending}");
        let id = pending["report_id"].as_str().unwrap();
        let lease = store
            .claim_report("report:0", "prefix-search")
            .await
            .unwrap()
            .unwrap();
        let generated =
            crate::sandbox_metrics::generate_fixture_with_collection(store.clone(), &lease, false)
                .await
                .unwrap();
        assert_eq!(
            generated.summary.as_ref().unwrap().sandbox_count,
            u64::from(expected)
        );
        store.finish_report(&lease, generated, true).await.unwrap();
        let (status, rows) = request(
            &store,
            "GET",
            &format!("{path}/{id}/sandboxes"),
            "owner-1",
            false,
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(status, 200, "{search}: {rows}");
        let items = rows["items"].as_array().unwrap();
        assert_eq!(items.len(), usize::from(expected), "{search}");
        if expected {
            assert_eq!(items[0]["sandbox_id"], own.sandbox_id.as_str());
        }
    }
    store.db.drop().await.unwrap();
}
