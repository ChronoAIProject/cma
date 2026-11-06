use super::*;
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn metrics_over_twenty_thousand_sandboxes_generate_full_cohort_and_indexed_pages() {
    let mut window = super::circuit_capture::ScaleWindow::start();
    let store = fixture().await;
    window.attach(&store);
    window.phase("seed", 0);
    let at = now();
    let from = (at - 3600) / 60 * 60;
    const COUNT: usize = 20_001;
    // The fixture represents one finalized minute, with no runtime or Agent
    // polling. Full daily reports reuse these same persisted minute chunks.
    for offset in (0..COUNT).step_by(200) {
        let states: Vec<_> = (offset..(offset + 200).min(COUNT))
            .map(|n| state(n, from))
            .collect();
        seed(&store, &states).await;
        let metrics: Vec<_> = states
            .iter()
            .enumerate()
            .map(|(n, s)| {
                let value = (offset + n + 1) as f64;
                let a = Aggregate {
                    integral: value * 60.,
                    display_integral: value * 60.,
                    covered_seconds: 60.,
                    expected_seconds: 60.,
                    min: Some(value),
                    max: Some(value),
                    latest: Some(value),
                    observed_at: Some((from + 30) as f64),
                    available_from: Some(from as f64),
                    ..Default::default()
                };
                MetricRollup {
                    schema_version: 1,
                    algorithm_version: ALGORITHM_VERSION,
                    source: SOURCE.into(),
                    sandbox_id: s.sandbox_id.to_string(),
                    owner_subject: s.owner.to_string(),
                    day: from / 86400 * 86400,
                    chunk: ((from % 86400) / 3600) as u16,
                    mapping_revision: 0,
                    cutoff: from + 60,
                    source_complete: true,
                    minutes: vec![Minute {
                        from,
                        to: from + 60,
                        metrics: [a.clone(), a.clone(), a],
                    }],
                }
            })
            .collect();
        store.save_metric_rollups(&metrics).await.unwrap();
    }
    let query = ReportInput {
        from: Some(from),
        to: Some(from + 60),
        ..Default::default()
    }
    .validate(at)
    .unwrap();
    window.phase("submit", 0);
    let report = store
        .submit(scope("operator", true), query, at)
        .await
        .unwrap();
    window.phase("claim", 0);
    let lease = store
        .claim_report("report:0", "large-fixture")
        .await
        .unwrap()
        .unwrap();
    window.phase("explain", 0);
    let rss_before = resident_kib();
    let explain = store.db.run_command(doc!{"explain":{"find":super::super::super::sandbox_metric_rollups::COLLECTION,
        "filter":{"sandbox_id":{"$in":["sbx_00000000000000000000000000000001"]},"day":{"$gte":(from/86400*86400) as i64,"$lte":(from/86400*86400) as i64}}},"verbosity":"executionStats"}).await.unwrap();
    assert!(!explain.to_string().contains("COLLSCAN"));
    assert!(
        explain
            .get_document("executionStats")
            .unwrap()
            .get_i64("totalDocsExamined")
            .or_else(|_| explain
                .get_document("executionStats")
                .unwrap()
                .get_i32("totalDocsExamined")
                .map(i64::from))
            .unwrap()
            <= 1
    );
    window.phase("generate", 0);
    let start = std::time::Instant::now();
    let generated = crate::sandbox_metrics::generate_fixture(store.clone(), &lease)
        .await
        .unwrap();
    let elapsed = start.elapsed();
    let rss_after = resident_kib();
    eprintln!(
        "metrics_scale generation_ms={} rss_before_kib={rss_before} rss_after_kib={rss_after}",
        elapsed.as_millis()
    );
    assert!(
        rss_after < 512 * 1024,
        "report process exceeds 512 MiB RSS fixture budget"
    );
    assert!(elapsed < Duration::from_secs(60));
    let summary = generated.summary.as_ref().unwrap();
    assert_eq!(summary.sandbox_count, COUNT as u64);
    assert_eq!(summary.running_count, COUNT as u64);
    assert_eq!(summary.allocated_period.uptime_seconds, COUNT as f64 * 60.);
    let expected = (COUNT * (COUNT + 1) / 2) as f64;
    assert_eq!(summary.measured.as_ref().unwrap().cpu.mean, Some(expected));
    window.phase("finish", 0);
    store
        .finish_report(&lease, generated.clone(), true)
        .await
        .unwrap();
    window.phase("reauthorize", 0);
    let authorization_start = std::time::Instant::now();
    store.reauthorize(&generated).await.unwrap();
    eprintln!(
        "metrics_scale authorization_ms={}",
        authorization_start.elapsed().as_millis()
    );
    assert!(authorization_start.elapsed() < Duration::from_secs(2));
    window.phase("first_page", 1);
    let first = store
        .page(
            &generated,
            PageQuery {
                sort: Sort::CpuMeanCores,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(first.items.len(), 50);
    assert_eq!(first.total_count, COUNT as u64);
    assert_eq!(first.items[0].sandbox_id, format!("sbx_{:032x}", COUNT - 1));
    let mut cursor = None;
    let mut count = 0;
    let mut page_number = 0;
    loop {
        page_number += 1;
        window.phase("paginate", page_number);
        let page = store
            .page(
                &generated,
                PageQuery {
                    limit: Some(200),
                    cursor,
                    sort: Sort::CpuMeanCores,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        count += page.items.len();
        cursor = page.next;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(count, COUNT);
    window.phase("series", 0);
    let chart_start = std::time::Instant::now();
    let series = store
        .series(
            &generated,
            SeriesQuery {
                metric: Metric::Cpu,
                group_by: GroupBy::Owner,
                series_id: None,
                top: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(series.lines.len(), 11);
    assert!(series.lines.iter().any(|l| l.id == "other"));
    assert_eq!(
        series
            .lines
            .iter()
            .map(|l| l.points[0].mean.unwrap())
            .sum::<f64>(),
        expected
    );
    window.phase("owners", 1);
    let owners = store
        .owners(
            &generated,
            OwnerQuery {
                limit: Some(5),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(owners.items.len(), 5);
    assert!(matches!(
        store
            .owners(
                &generated,
                OwnerQuery {
                    search: "other".into(),
                    cursor: owners.next.clone(),
                    ..Default::default()
                }
            )
            .await,
        Err(MetricsError::Invalid(_))
    ));
    window.phase("owners", 2);
    let next = store
        .owners(
            &generated,
            OwnerQuery {
                limit: Some(200),
                cursor: owners.next,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(next.items.len(), 7);
    window.phase("verify_persistence", 0);
    let saved = store
        .collection(JOBS)
        .find_one(doc! {"_id":&report.report_id})
        .await
        .unwrap()
        .unwrap();
    eprintln!(
        "metrics_fixture sandboxes={COUNT} generation_ms={} chart_ms={} cache_bytes={}",
        elapsed.as_millis(),
        chart_start.elapsed().as_millis(),
        saved.get_i64("bytes").unwrap()
    );
    assert_eq!(
        store
            .collection(super::super::super::sandbox_usage::EVENTS)
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    let unchanged = store
        .collection(super::super::super::sandbox_usage::STATES)
        .find_one(doc! {"_id":state(COUNT-1,from).sandbox_id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        super::super::super::sandbox_usage::read_state(&unchanged).unwrap(),
        state(COUNT - 1, from)
    );
    window.phase("cleanup", 0);
    store.db.drop().await.unwrap();
    window.assert_no_pressure();
    window.phase("complete", page_number);
}

fn resident_kib() -> u64 {
    let output = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout)
        .unwrap()
        .trim()
        .parse()
        .unwrap()
}
