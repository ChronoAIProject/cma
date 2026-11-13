use super::*;
use axum::body::to_bytes;
use layer2_core::Capability;
#[tokio::test]
async fn retained_admin_view_refuses_anonymous_and_owner_only_callers_before_store_work() {
    let store = Arc::new(tests::Store::default());
    for method in ["GET", "HEAD"] {
        for (caller, status) in [
            (None, StatusCode::UNAUTHORIZED),
            (
                Some(("owner-a", &[Capability::UseOwnSandboxes][..])),
                StatusCode::FORBIDDEN,
            ),
        ] {
            // Invalid input must not take precedence over current authority.
            for query in ["", "?window=invalid"] {
                let path = format!("/api/v1/admin/usage/metrics/view{query}");
                let response = tests::call(&store, method, &path, caller, "").await;
                assert_eq!(response.status(), status);
                assert!(store.saved.lock().unwrap().is_none());
                assert_eq!(
                    store.view_reads.load(std::sync::atomic::Ordering::SeqCst),
                    0
                );
            }
        }
    }
}

#[tokio::test]
async fn retained_summary_routes_preserve_scope_headers_and_head_semantics() {
    for (prefix, capability) in [
        ("/api/v1/admin/usage/metrics", Capability::ReadAllSandboxes),
        ("/api/v1/usage/metrics", Capability::UseOwnSandboxes),
    ] {
        let store = Arc::new(tests::Store::default());
        let path = format!("{prefix}/view?window=24h");
        let response =
            tests::call(&store, "GET", &path, Some(("owner-a", &[capability])), "").await;
        assert_eq!(response.status(), 200);
        assert_eq!(
            response.headers()[header::CACHE_CONTROL],
            "private, no-store"
        );
        let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        let view: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(view["report"]["status"], "ready");
        assert!(view["report"]["summary"].is_object());
        assert_eq!(view["stale"], false);
        for path in [
            path,
            format!("{prefix}/reports/umr_00000000000000000000000000000001"),
        ] {
            let response =
                tests::call(&store, "HEAD", &path, Some(("owner-a", &[capability])), "").await;
            assert_eq!(response.status(), 200);
            assert!(to_bytes(response.into_body(), 1024 * 1024)
                .await
                .unwrap()
                .is_empty());
        }
        store
            .revoked
            .store(true, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(
            tests::call(
                &store,
                "GET",
                &format!("{prefix}/view"),
                Some(("owner-a", &[capability])),
                ""
            )
            .await
            .status(),
            404
        );
        assert_eq!(
            tests::call(
                &store,
                "GET",
                &format!("{prefix}/view"),
                Some(("owner-a", &[])),
                ""
            )
            .await
            .status(),
            403
        );
    }
}
#[tokio::test]
async fn retained_view_rejects_unbounded_or_malformed_filter_input_before_work() {
    let store = Arc::new(tests::Store::default());
    for query in [
        "window=unexpected",
        "window=custom",
        "window=24h&from=1",
        "filters=%7B%22unknown%22%3Atrue%7D",
    ] {
        let response = tests::call(
            &store,
            "GET",
            &format!("/api/v1/admin/usage/metrics/view?{query}"),
            Some(("owner-a", &[Capability::ReadAllSandboxes])),
            "",
        )
        .await;
        assert_eq!(response.status(), 400);
        assert!(store.saved.lock().unwrap().is_none());
    }
}

#[tokio::test]
async fn retained_summary_upper_bound_fits_one_mib_with_maximum_filter_and_numeric_fields() {
    use layer2_core::sandbox_metrics::*;
    let store = Arc::new(tests::Store::default());
    // Conservative bound: even a whole 64 KiB provider response decoded as
    // a subject, with worst JSON escaping, leaves room below the route cap.
    // Real valid provider evidence needs additional fields, so is smaller.
    let principal = "\u{1}".repeat(64 * 1024);
    let at = now();
    let filters = Filters {
        owner: Some("\"".repeat(256)),
        runtime_version: Some("\"".repeat(512)),
        search: "\"".repeat(128),
        statuses: vec![
            Status::Provisioning,
            Status::Installing,
            Status::Active,
            Status::Pausing,
            Status::Paused,
            Status::Reviving,
            Status::CredentialRotating,
            Status::Rotating,
            Status::Retiring,
            Status::Retired,
            Status::Failed,
        ],
        ..Default::default()
    };
    let query = ReportInput {
        filters: filters.clone(),
        ..Default::default()
    }
    .validate(at)
    .unwrap();
    let mut report = store
        .submit(
            Scope {
                principal: principal.clone(),
                admin: true,
            },
            query,
            at,
        )
        .await
        .unwrap();
    let mut measured = Measurement::project(&Aggregate::default(), Metric::Cpu, at, true, 0.);
    measured.source = SOURCE.into();
    measured.observed_at = Some(f64::MAX);
    measured.covered_seconds = f64::MAX;
    measured.expected_seconds = f64::MAX;
    measured.unknown_mapping_seconds = f64::MAX;
    measured.coverage_ratio = Some(f64::MAX);
    measured.available_from = Some(f64::MAX);
    measured.latest = Some(f64::MAX);
    measured.last_observed = Some(f64::MAX);
    measured.mean = Some(f64::MAX);
    measured.max = Some(f64::MAX);
    measured.observed_hours = Some(f64::MAX);
    measured.percent_of_limit = Some(f64::MAX);
    measured.filesystem_capacity_bytes = Some(f64::MAX);
    let mut summary = Summary {
        sandbox_count: u64::MAX,
        running_count: u64::MAX,
        excluded_count: u64::MAX,
        measured: Some(Measured {
            cpu: measured.clone(),
            memory: measured.clone(),
            disk: measured,
        }),
        infrastructure: Some(Infrastructure {
            state: "source_unavailable".into(),
            unproven_count: u64::MAX,
            unassigned_pool: [Some(f64::MAX); 3],
            unattributed: [Some(f64::MAX); 3],
            observed_at: Some(u64::MAX),
        }),
        ..Default::default()
    };
    summary.total_allocated_capacity = Capacity {
        cpu_cores: f64::MAX,
        memory_bytes: f64::MAX,
        disk_bytes: f64::MAX,
    };
    summary.covered_allocated_capacity = summary.total_allocated_capacity.clone();
    summary.allocated_period = AllocatedPeriod {
        disk_retained_seconds: f64::MAX,
        uptime_seconds: f64::MAX,
        cpu_core_hours: f64::MAX,
        memory_gib_hours: f64::MAX,
        disk_gib_hours: f64::MAX,
        complete: false,
        projected_tail_seconds: u64::MAX,
        unknown_seconds: u64::MAX,
    };
    report.summary = Some(summary);
    report.allocation_watermark = "f".repeat(64);
    report.mapping_revision = u64::MAX;
    report.available_from = Some(u64::MAX);
    report.tracking_started_at = Some(u64::MAX);
    report.warnings = [
        "source_unavailable",
        "measurement_coverage_incomplete",
        "allocation_history_incomplete",
        "no_finalized_samples",
    ]
    .map(str::to_owned)
    .to_vec();
    *store.saved.lock().unwrap() = Some(report);
    let filters = serde_json::to_string(&filters).unwrap();
    let encoded: String = filters.bytes().map(|byte| format!("%{byte:02X}")).collect();
    let response = tests::call(
        &store,
        "GET",
        &format!("/api/v1/admin/usage/metrics/view?filters={encoded}"),
        Some((&principal, &[Capability::ReadAllSandboxes])),
        "",
    )
    .await;
    assert_eq!(response.status(), 200);
    let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        json["report"]["summary"]["sandbox_count"],
        u64::MAX.to_string()
    );
    assert!(json.get("members").is_none());
    eprintln!(
        "retained_view_bound serialized_bytes={} subject_bytes={} filter_json_bytes={}",
        body.len(),
        principal.len(),
        filters.len()
    );
}
