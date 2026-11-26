use super::*;

#[tokio::test]
async fn initial_routes_refuse_auth_before_invalid_input_and_never_fanout() {
    for prefix in ["/api/v1/usage/metrics", "/api/v1/admin/usage/metrics"] {
        for suffix in [
            "/reports".into(),
            "/view".into(),
            format!("/reports/{REPORT}"),
        ] {
            let store = Store::new("", 1);
            let path = format!("{prefix}/initial{suffix}?unknown=true");
            for (who, status) in [(None, 401), (Some(("owner-a", &[][..])), 403)] {
                assert_eq!(
                    call(&store, &path, who, json!({"bad":true})).await.status(),
                    status
                );
            }
            assert_eq!(store.admissions.load(Ordering::SeqCst), 0);
            assert_eq!(store.auth.load(Ordering::SeqCst), 0);
        }
    }
}

#[tokio::test]
async fn initial_routes_refuse_overbroad_input_before_work() {
    for suffix in [
        "/reports",
        "/view",
        "/reports/umr_00000000000000000000000000000001",
    ] {
        let store = Store::new("", 1);
        let path = format!("/api/v1/usage/metrics/initial{suffix}");
        for projection in [
            json!({"first_page":{"limit":0}}),
            json!({"first_page":{"limit":51}}),
            json!({"first_page":{"cursor":"next"}}),
            json!({"first_page":{"report_id":REPORT}}),
            json!({"total_metrics":["cpu","cpu"]}),
            json!({"total_metrics":["other"]}),
            json!({"group_by":"owner"}),
            json!({"total_metrics":[{"metric":"cpu","series_id":"other"}]}),
        ] {
            let mut input = request(suffix);
            if suffix == "/reports" || suffix == "/view" {
                input["projection"] = projection;
            } else {
                input = projection;
            }
            assert_eq!(
                call(&store, &path, Some(("owner-a", OWNER)), input)
                    .await
                    .status(),
                400
            );
        }
        for (uri, input) in [
            (path.clone(), json!({"unknown":"x".repeat(16384)})),
            (format!("{path}?cursor=next"), request(suffix)),
            (path.clone(), json!({"report_id":REPORT})),
        ] {
            assert_eq!(
                call(&store, &uri, Some(("owner-a", OWNER)), input)
                    .await
                    .status(),
                400
            );
        }
        assert_eq!(store.admissions.load(Ordering::SeqCst), 0);
        assert_eq!(store.pages.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn initial_reads_preserve_foreign_scope_expiry_and_legacy_polling() {
    let store = Store::new("", 0);
    let prefix = "/api/v1/usage/metrics";
    assert_eq!(
        call(
            &store,
            &format!("{prefix}/initial/reports"),
            Some(("owner-a", OWNER)),
            request("/reports")
        )
        .await
        .status(),
        200
    );
    for (who, status) in [
        (Some(("owner-b", OWNER)), 404),
        (Some(("owner-a", &[][..])), 403),
    ] {
        assert_eq!(
            call(
                &store,
                &format!("{prefix}/initial/reports/{REPORT}"),
                who,
                projection()
            )
            .await
            .status(),
            status
        );
    }
    let mut req = Request::builder()
        .uri(format!("{prefix}/reports/{REPORT}"))
        .body(Body::empty())
        .unwrap();
    req.extensions_mut()
        .insert(crate::AuthenticatedSubject(SubjectId("owner-a".into())));
    req.extensions_mut()
        .insert(Authorization::new(OWNER.iter().copied()));
    let response = sandbox_metrics_router(MetricsService::new(store.clone()))
        .oneshot(req)
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let value = body(response).await;
    assert_eq!(value["report_id"], REPORT);
    assert!(value.get("initial").is_none());
    store
        .base
        .saved
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .expires_at = 0;
    let response = call(
        &store,
        &format!("{prefix}/initial/reports/{REPORT}"),
        Some(("owner-a", OWNER)),
        projection(),
    )
    .await;
    assert_eq!(response.status(), 410);
    assert_eq!(body(response).await["code"], "report_expired");
}

#[tokio::test]
async fn maximum_initial_projection_serializes_complete_body_under_ceiling() {
    let to = now() / 300 * 300 - 300;
    for measured in [false, true] {
        let store = Store::new("wide", 50);
        let metrics = if measured { projection() } else { json!({}) };
        let response = call(
            &store,
            "/api/v1/admin/usage/metrics/initial/reports",
            Some(("owner-a", ADMIN)),
            json!({"query":{"from":to-604800,"to":to,"step_seconds":300},"projection":metrics}),
        )
        .await;
        assert_eq!(response.status(), 200);
        let bytes = to_bytes(response.into_body(), encoding::MAX_INITIAL_BYTES)
            .await
            .unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            value["initial"]["rows"]["items"].as_array().unwrap().len(),
            50
        );
        assert_eq!(
            value["initial"]["charts"].as_array().unwrap().len(),
            if measured { 3 } else { 0 }
        );
        if measured {
            for chart in value["initial"]["charts"].as_array().unwrap() {
                assert_eq!(chart["lines"][0]["points"].as_array().unwrap().len(), 2016);
                assert_eq!(chart["lines"][0]["points"][0]["mean"], "9007199254740992");
            }
        }
        assert!(value["initial"]["rows"]["items"][0]["measured"]["cpu"]
            .get("mean_cores")
            .is_some());
        eprintln!("initial_full_body measured={measured} rows=50 total_points={} bytes={} ownership_checks={}",if measured {6048} else {0},bytes.len(),store.auth.load(Ordering::SeqCst));
    }
}

#[test]
fn initial_openapi_documents_strict_bounded_inputs_and_separate_routes() {
    let doc = serde_json::to_value(document()).unwrap();
    let schemas = &doc["components"]["schemas"];
    assert_eq!(
        schemas["UsageInitialProjection"]["additionalProperties"],
        false
    );
    assert_eq!(
        schemas["UsageInitialProjection"]["properties"]["first_page"]["properties"]["limit"]
            ["maximum"],
        50
    );
    assert_eq!(
        schemas["UsageInitialProjection"]["properties"]["total_metrics"]["uniqueItems"],
        true
    );
    assert_eq!(
        schemas["UsageInitialSeries"]["properties"]["lines"]["items"]["properties"]["points"]
            ["maxItems"],
        2016
    );
    for prefix in ["/api/v1/usage/metrics", "/api/v1/admin/usage/metrics"] {
        for suffix in [
            "/initial/reports",
            "/initial/view",
            "/initial/reports/{report}",
        ] {
            let operation = &doc["paths"][format!("{prefix}{suffix}")]["post"];
            assert!(operation["requestBody"]["required"].as_bool().unwrap());
            for status in ["200", "202", "503"] {
                assert!(operation["responses"][status].is_object());
            }
        }
        assert!(doc["paths"][format!("{prefix}/reports/{{report}}")]["get"].is_object());
    }
}
