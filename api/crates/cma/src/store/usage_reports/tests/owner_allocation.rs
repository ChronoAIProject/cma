use super::*;
use layer2_core::sandbox_metrics::OwnerContextAccess;

fn query(metric: Metric, limit: u16, cursor: Option<String>) -> OwnerAllocationQuery {
    OwnerAllocationQuery {
        metric,
        limit: Some(limit),
        cursor,
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn metrics_owner_allocations_full_cohort_at_400_and_20001_is_bounded_and_cursor_bound() {
    for count in [400, 20_001] {
        let per_owner = if count == 400 { 2 } else { 1 };
        let store = fixture().await;
        let at = now();
        let report = store
            .submit(
                scope("operator", true),
                ReportInput::default().validate(at).unwrap(),
                at,
            )
            .await
            .unwrap();
        let lease = store
            .claim_report("report:0", "owner-allocation-test")
            .await
            .unwrap()
            .unwrap();
        for offset in (0..count).step_by(200) {
            let states: Vec<_> = (offset..(offset + 200).min(count))
                .map(|n| {
                    let mut state = state(n, at - 3600);
                    // Cover multi-Sandbox totals and the worst case of one owner per Sandbox.
                    state.owner = SubjectId(format!("owner-{:06}", n / per_owner));
                    state
                })
                .collect();
            seed(&store, &states).await;
            let rows: Vec<_> = states
                .iter()
                .enumerate()
                .map(|(n, state)| {
                    let mut row = fixture_row(state);
                    row.allocated_period.cpu_core_hours = 1.;
                    row.allocated_period.memory_gib_hours = 2.;
                    row.allocated_period.disk_gib_hours = 3.;
                    row.allocated_period.complete = (offset + n) % 3 != 0;
                    row
                })
                .collect();
            store
                .write_report_chunk(&lease, &rows, vec![])
                .await
                .unwrap();
        }
        store
            .finish_report(&lease, report.clone(), true)
            .await
            .unwrap();
        store.collection("users").insert_many([
            doc! {"_id":"owner-000000","display_name":"Alex Chen","first_seen_at":"2026-01-01T00:00:00Z"},
            doc! {"_id":"owner-000001","display_name":"Alex Chen","first_seen_at":"2026-02-01T00:00:00Z"},
            doc! {"_id":"owner-000002","display_name":"Name-91892afa-5fa5-4f4f-9100-2b8d986d2f71"},
        ]).await.unwrap();
        let started = std::time::Instant::now();
        let first = store
            .owner_allocations(
                &report,
                query(Metric::Cpu, 200, None),
                OwnerContextAccess::DateOnly,
            )
            .await
            .unwrap();
        assert_eq!(first.items.len(), 200);
        assert_eq!(first.total_count, count.div_ceil(per_owner) as u64);
        assert_eq!(first.items[0].allocated_hours, per_owner as f64);
        assert_eq!(first.items[0].sandbox_count, per_owner as u64);
        assert!(!first.items[0].complete);
        assert_eq!(first.items[0].label, "Alex Chen");
        assert_eq!(
            first.items[0].disambiguator.as_deref(),
            Some("Joined 2026-01-01")
        );
        assert_eq!(
            first.items[1].disambiguator.as_deref(),
            Some("Joined 2026-02-01")
        );
        assert_eq!(first.items[2].label, "Unnamed owner");
        assert!(started.elapsed() < Duration::from_secs(2));
        let token = store
            .owner_allocations(
                &report,
                query(Metric::Cpu, 2, None),
                OwnerContextAccess::DateOnly,
            )
            .await
            .unwrap()
            .next
            .unwrap();
        for (changed, q) in [
            (
                report.clone(),
                query(Metric::Memory, 2, Some(token.clone())),
            ),
            (
                {
                    let mut r = report.clone();
                    r.scope.principal = "other".into();
                    r
                },
                query(Metric::Cpu, 2, Some(token.clone())),
            ),
            (
                {
                    let mut r = report.clone();
                    r.report_id.push('1');
                    r
                },
                query(Metric::Cpu, 2, Some(token.clone())),
            ),
            (
                report.clone(),
                query(Metric::Cpu, 2, Some(format!("{token}x"))),
            ),
        ] {
            assert!(matches!(
                store
                    .owner_allocations(&changed, q, OwnerContextAccess::DateOnly)
                    .await,
                Err(MetricsError::Invalid(_))
            ));
        }
        let http_started = std::time::Instant::now();
        let (status, body) = super::allocated::request(
            &store,
            "GET",
            &format!(
                "/api/v1/admin/usage/metrics/reports/{}/owner-allocations?metric=cpu&limit=5",
                report.report_id
            ),
            "operator",
            true,
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(body["total_count"], count.div_ceil(per_owner));
        assert_eq!(body["items"][0]["label"], "Alex Chen");
        assert!(http_started.elapsed() < Duration::from_secs(5));
        eprintln!(
            "owner allocation sandboxes={count} HTTP authorization+aggregation_ms={}",
            http_started.elapsed().as_millis()
        );
        let mut cursor = None;
        let mut subjects = std::collections::BTreeSet::new();
        let mut total = 0.;
        let mut previous = f64::INFINITY;
        loop {
            let page = store
                .owner_allocations(
                    &report,
                    query(Metric::Cpu, 200, cursor),
                    OwnerContextAccess::DateOnly,
                )
                .await
                .unwrap();
            assert!(page.items.len() <= 200);
            for item in page.items {
                assert!(item.allocated_hours <= previous);
                previous = item.allocated_hours;
                total += item.allocated_hours;
                assert!(subjects.insert(item.subject));
                assert!(!item.label.contains("91892afa"));
            }
            cursor = page.next;
            if cursor.is_none() {
                break;
            }
        }
        assert_eq!(subjects.len(), count.div_ceil(per_owner));
        assert_eq!(total, count as f64);
        for (metric, expected) in [
            (Metric::Memory, 2. * per_owner as f64),
            (Metric::Disk, 3. * per_owner as f64),
        ] {
            assert_eq!(
                store
                    .owner_allocations(
                        &report,
                        query(metric, 5, None),
                        OwnerContextAccess::DateOnly
                    )
                    .await
                    .unwrap()
                    .items[0]
                    .allocated_hours,
                expected
            );
        }
        assert_eq!(
            store
                .collection(JOBS)
                .count_documents(doc! {})
                .await
                .unwrap(),
            1
        );
        eprintln!(
            "owner allocation sandboxes={count} owners={} paging_ms={}",
            subjects.len(),
            started.elapsed().as_millis()
        );
        store.db.drop().await.unwrap();
    }
}
