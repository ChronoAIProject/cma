use super::*;
use layer2_core::sandbox_metrics::*;

// Exercise the public serialized descriptor with maximum accepted query lengths,
// every status/filter, every optional summary field, and full-width numbers.
// Report generation only emits four fixed warnings; it never embeds member rows.
#[tokio::test]
async fn full_descriptor_with_maximum_query_fits_the_small_lane_without_truncation() {
    let at = 1_800_000_000;
    let query = ReportInput {
        from: Some(at - 604_800),
        to: Some(at),
        step_seconds: Some(300),
        filters: Filters {
            owner: Some("o".repeat(256)),
            runtime_version: Some("r".repeat(512)),
            search: "s".repeat(128),
            sandbox_id: Some(format!("sbx_{}", "a".repeat(32))),
            agent_id: Some(format!("agt_{}", "b".repeat(32))),
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
        },
    }
    .validate(at)
    .unwrap();
    let measurement = Measurement {
        state: MetricState::Unattributed,
        source: "source_unavailable".into(),
        observed_at: Some(f64::MAX),
        covered_seconds: f64::MAX,
        expected_seconds: f64::MAX,
        unknown_mapping_seconds: f64::MAX,
        coverage_ratio: Some(f64::MAX),
        available_from: Some(f64::MAX),
        latest: Some(f64::MAX),
        last_observed: Some(f64::MAX),
        mean: Some(f64::MAX),
        max: Some(f64::MAX),
        observed_hours: Some(f64::MAX),
        percent_of_limit: Some(f64::MAX),
        filesystem_capacity_bytes: Some(f64::MAX),
        exceeds_capacity: true,
    };
    let capacity = Capacity {
        cpu_cores: f64::MAX,
        memory_bytes: f64::MAX,
        disk_bytes: f64::MAX,
    };
    let report = Report {
        schema_version: u32::MAX,
        report_id: format!("rpt_{}", "a".repeat(32)),
        status: ReportStatus::Ready,
        report_complete: true,
        scope: Scope {
            principal: "p".repeat(256),
            admin: true,
        },
        effective_range: query.range(),
        query,
        as_of: u64::MAX,
        measurement_cutoff: u64::MAX,
        expires_at: u64::MAX,
        allocation_watermark: "f".repeat(64),
        mapping_revision: u64::MAX,
        available_from: Some(u64::MAX),
        tracking_started_at: Some(u64::MAX),
        warnings: [
            "source_unavailable",
            "measurement_coverage_incomplete",
            "allocation_history_incomplete",
            "no_finalized_samples",
        ]
        .map(String::from)
        .into(),
        source_health: "source_unavailable".into(),
        retry_after_seconds: u16::MAX,
        poll_url: format!("/api/v1/admin/usage/metrics/reports/rpt_{}", "a".repeat(32)),
        summary: Some(Summary {
            sandbox_count: u64::MAX,
            running_count: u64::MAX,
            excluded_count: u64::MAX,
            allocated_period: AllocatedPeriod {
                disk_retained_seconds: f64::MAX,
                uptime_seconds: f64::MAX,
                cpu_core_hours: f64::MAX,
                memory_gib_hours: f64::MAX,
                disk_gib_hours: f64::MAX,
                complete: false,
                projected_tail_seconds: u64::MAX,
                unknown_seconds: u64::MAX,
            },
            total_allocated_capacity: capacity.clone(),
            covered_allocated_capacity: capacity,
            measured: Some(Measured {
                cpu: measurement.clone(),
                memory: measurement.clone(),
                disk: measurement,
            }),
            infrastructure: Some(Infrastructure {
                state: "source_unavailable".into(),
                unproven_count: u64::MAX,
                unassigned_pool: [Some(f64::MAX); 3],
                unattributed: [Some(f64::MAX); 3],
                observed_at: Some(u64::MAX),
            }),
        }),
    };
    let bytes = serde_json::to_vec(&report).unwrap();
    assert!(bytes.len() < 16 * 1024, "descriptor bytes={}", bytes.len());
    let expected = bytes.clone();
    let router = protected(
        Router::new().route(
            "/api/v1/admin/usage/metrics/reports/{report}",
            get(move || {
                let bytes = bytes.clone();
                async move { bytes }
            }),
        ),
        ReadBudget::new(false),
    );
    let response = request(
        &router,
        Method::GET,
        "/api/v1/admin/usage/metrics/reports/r",
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        to_bytes(response.into_body(), Lane::Report.bytes())
            .await
            .unwrap(),
        expected
    );
}
