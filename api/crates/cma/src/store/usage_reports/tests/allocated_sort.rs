use super::*;

const SORTS: [Sort; 3] = [
    Sort::AllocatedCpuCoreHours,
    Sort::AllocatedMemoryGibHours,
    Sort::AllocatedDiskGibHours,
];

fn rows(mixed: bool) -> Vec<Row> {
    let values = [
        8.165,
        14.729,
        37.688,
        47.935,
        0.,
        8.165,
        8.1654,
        0.46694444444444444,
    ];
    (0..63)
        .map(|n| {
            let mut row = fixture_row(&state(n, now() - 3600));
            row.allocated_period = AllocatedPeriod {
                cpu_core_hours: values[n % values.len()],
                memory_gib_hours: values[(n * 3 + 1) % values.len()],
                disk_gib_hours: values[(n * 5 + 2) % values.len()],
                complete: mixed && n % 2 == 0,
                unknown_seconds: if mixed && n % 2 == 0 { 0 } else { 600 },
                projected_tail_seconds: 120,
                ..Default::default()
            };
            if n >= 61 {
                row.allocated_period.cpu_core_hours = f64::NAN;
                row.allocated_period.memory_gib_hours = f64::INFINITY;
                row.allocated_period.disk_gib_hours = f64::NEG_INFINITY;
            }
            row
        })
        .collect()
}

fn value(row: &Row, sort: Sort) -> f64 {
    match sort {
        Sort::AllocatedCpuCoreHours => row.allocated_period.cpu_core_hours,
        Sort::AllocatedMemoryGibHours => row.allocated_period.memory_gib_hours,
        Sort::AllocatedDiskGibHours => row.allocated_period.disk_gib_hours,
        _ => unreachable!(),
    }
}

#[test]
fn allocated_sort_encoding_keeps_zero_precision_and_partial_values_but_not_nonfinite() {
    for mixed in [false, true] {
        for row in rows(mixed) {
            let (sorts, nulls) = super::super::work::row_sort_keys(&row);
            for sort in SORTS {
                let subtotal = value(&row, sort);
                if subtotal.is_finite() {
                    assert_eq!(sorts.get_f64(sort.key()).unwrap(), subtotal);
                    assert!(!nulls.get_bool(sort.key()).unwrap());
                } else {
                    assert_eq!(sorts.get(sort.key()), Some(&mongodb::bson::Bson::Null));
                    assert!(nulls.get_bool(sort.key()).unwrap());
                }
            }
            assert_eq!(sorts.get_f64(Sort::CpuMeanCores.key()).unwrap(), 1.);
            assert_eq!(row.measured.cpu.coverage_ratio, Some(1.));
        }
    }
}

fn expected_ids(rows: &[Row], sort: Sort, order: Order) -> Vec<String> {
    let mut sorted: Vec<_> = rows.iter().collect();
    sorted.sort_by(|a, b| {
        let (left, right) = (value(a, sort), value(b, sort));
        let tie = a.sandbox_id.cmp(&b.sandbox_id);
        let numeric = if left.is_finite() && right.is_finite() {
            left.partial_cmp(&right).unwrap().then(tie)
        } else {
            tie
        };
        right
            .is_finite()
            .cmp(&left.is_finite())
            .then(if order == Order::Asc {
                numeric
            } else {
                numeric.reverse()
            })
    });
    sorted.into_iter().map(|r| r.sandbox_id.clone()).collect()
}

async fn finish(store: &ReportingStore, lease: &ReportLease, rows: &[Row]) -> Report {
    store.write_report_chunk(lease, rows, vec![]).await.unwrap();
    let mut report = lease.report.clone();
    report.summary = Some(Summary {
        sandbox_count: rows.len() as u64,
        ..Default::default()
    });
    store
        .finish_report(lease, report.clone(), true)
        .await
        .unwrap();
    store
        .report(&report.scope, &report.report_id, now())
        .await
        .unwrap()
}

async fn pages(store: &ReportingStore, report: &Report, sort: Sort, order: Order) -> Vec<Row> {
    let mut items = Vec::new();
    let mut cursor = None;
    loop {
        let page = store
            .page(
                report,
                PageQuery {
                    limit: Some(50),
                    cursor,
                    sort,
                    order,
                },
            )
            .await
            .unwrap();
        assert_eq!(page.total_count, 63);
        items.extend(page.items);
        cursor = page.next;
        if cursor.is_none() {
            break;
        }
        assert!(items.len() < 63);
    }
    assert_eq!(items.len(), 63);
    assert_eq!(
        items
            .iter()
            .map(|r| &r.sandbox_id)
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        63
    );
    items
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn allocated_sort_pages_use_partial_subtotals_with_ties_nulls_and_bound_cursors() {
    let store = fixture().await;
    for mixed in [false, true] {
        let mut query = ReportInput::default().validate(now()).unwrap();
        query.filters.search = format!("mixed-{mixed}");
        store
            .submit(scope("operator", true), query, now())
            .await
            .unwrap();
        let lease = store
            .claim_report("report:0", "allocated-sort")
            .await
            .unwrap()
            .unwrap();
        let original = rows(mixed);
        let report = finish(&store, &lease, &original).await;
        for sort in SORTS {
            for order in [Order::Asc, Order::Desc] {
                let actual = pages(&store, &report, sort, order).await;
                assert_eq!(
                    actual
                        .iter()
                        .map(|r| r.sandbox_id.clone())
                        .collect::<Vec<_>>(),
                    expected_ids(&original, sort, order),
                    "{mixed} {sort:?} {order:?}"
                );
                for row in &actual {
                    let source = original
                        .iter()
                        .find(|r| r.sandbox_id == row.sandbox_id)
                        .unwrap();
                    assert_eq!(
                        row.allocated_period.complete,
                        source.allocated_period.complete
                    );
                    assert_eq!(
                        row.allocated_period.unknown_seconds,
                        source.allocated_period.unknown_seconds
                    );
                    assert_eq!(row.allocated_period.projected_tail_seconds, 120);
                    assert_eq!(
                        row.measured.cpu.coverage_ratio,
                        source.measured.cpu.coverage_ratio
                    );
                }
                let first = store
                    .page(
                        &report,
                        PageQuery {
                            limit: Some(50),
                            sort,
                            order,
                            ..Default::default()
                        },
                    )
                    .await
                    .unwrap();
                for mutation in 0..5 {
                    let mut other = report.clone();
                    let mut page = PageQuery {
                        cursor: first.next.clone(),
                        sort,
                        order,
                        ..Default::default()
                    };
                    match mutation {
                        0 => page.sort = Sort::Name,
                        1 => {
                            page.order = if order == Order::Asc {
                                Order::Desc
                            } else {
                                Order::Asc
                            }
                        }
                        2 => other.report_id = "different-report".into(),
                        3 => other.scope.principal = "different-owner".into(),
                        4 => other.scope.admin = false,
                        _ => unreachable!(),
                    }
                    assert!(matches!(
                        store.page(&other, page).await,
                        Err(MetricsError::Invalid(_))
                    ));
                }
            }
        }
    }
    store.db.drop().await.unwrap();
}

async fn generate(store: &Arc<ReportingStore>, retained: bool, refresh: bool) -> Report {
    let principal = scope("operator", true);
    if retained {
        let mut selection = ViewInput::default().validate(now()).unwrap();
        selection.refresh = refresh;
        store.latest(&principal, &selection, now()).await.unwrap();
    } else {
        let mut query = ReportInput::default().validate(now()).unwrap();
        if refresh {
            query.from -= 60;
        }
        store.submit(principal.clone(), query, now()).await.unwrap();
    }
    let lease = store
        .claim_report("report:0", "snapshot-sort")
        .await
        .unwrap()
        .unwrap();
    let report =
        crate::sandbox_metrics::generate_fixture_with_collection(store.clone(), &lease, false)
            .await
            .unwrap();
    let id = report.report_id.clone();
    store.finish_report(&lease, report, true).await.unwrap();
    store.report(&principal, &id, now()).await.unwrap()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn allocated_sort_new_reports_and_views_preserve_legacy_cursors_until_expiry() {
    for retained in [false, true] {
        let store = if retained {
            super::retained_views::enabled().await
        } else {
            fixture().await
        };
        let at = now();
        let states: Vec<_> = (0..63)
            .map(|n| {
                let mut state = state(n, at - 7200);
                state.tracking_started_at = at - 3600;
                state.last_observed_at = at - 1800;
                state.complete_lifecycle = false;
                state.allocation.cpu_millis = ((n * 3 + 1) % 7) as u32 * 1000;
                state.allocation.memory_mib = ((n * 5 + 2) % 7) as u32 * 1024;
                state.allocation.disk_gib = ((n * 2 + 3) % 7) as u32;
                state
            })
            .collect();
        seed(&store, &states).await;
        let old = generate(&store, retained, false).await;
        // Recreate the old writer's persisted keys before any cursor is issued.
        let mut legacy = Document::new();
        for sort in SORTS {
            legacy.insert(format!("nulls.{}", sort.key()), true);
            legacy.insert(format!("sorts.{}", sort.key()), mongodb::bson::Bson::Null);
        }
        store
            .collection(ROWS)
            .update_many(doc! {"report_id":&old.report_id}, doc! {"$set":legacy})
            .await
            .unwrap();
        let frozen: Vec<Document> = store
            .collection(ROWS)
            .find(doc! {"report_id":&old.report_id})
            .sort(doc! {"sandbox_id":1})
            .await
            .unwrap()
            .try_collect()
            .await
            .unwrap();
        let mut first_pages = Vec::new();
        for sort in SORTS {
            for order in [Order::Asc, Order::Desc] {
                let page = store
                    .page(
                        &old,
                        PageQuery {
                            limit: Some(50),
                            sort,
                            order,
                            ..Default::default()
                        },
                    )
                    .await
                    .unwrap();
                first_pages.push((sort, order, page));
            }
        }
        let new = generate(&store, retained, true).await;
        assert_ne!(new.report_id, old.report_id);
        let source = pages(&store, &new, Sort::AllocatedCpuCoreHours, Order::Asc).await;
        assert!(source
            .iter()
            .all(|r| !r.allocated_period.complete && r.allocated_period.unknown_seconds > 0));
        for (sort, order, first) in first_pages {
            let next = store
                .page(
                    &old,
                    PageQuery {
                        cursor: first.next.clone(),
                        sort,
                        order,
                        ..Default::default()
                    },
                )
                .await
                .unwrap();
            assert!(next.next.is_none());
            let ids: Vec<_> = first
                .items
                .iter()
                .chain(&next.items)
                .map(|r| r.sandbox_id.clone())
                .collect();
            let mut legacy_ids: Vec<_> = states.iter().map(|s| s.sandbox_id.to_string()).collect();
            if order == Order::Desc {
                legacy_ids.reverse();
            }
            assert_eq!(ids, legacy_ids);
            let new_rows = pages(&store, &new, sort, order).await;
            assert_eq!(
                new_rows
                    .iter()
                    .map(|r| r.sandbox_id.clone())
                    .collect::<Vec<_>>(),
                expected_ids(&source, sort, order)
            );
            assert!(matches!(
                store
                    .page(
                        &new,
                        PageQuery {
                            cursor: first.next,
                            sort,
                            order,
                            ..Default::default()
                        }
                    )
                    .await,
                Err(MetricsError::Invalid(_))
            ));
        }
        let unchanged: Vec<Document> = store
            .collection(ROWS)
            .find(doc! {"report_id":&old.report_id})
            .sort(doc! {"sandbox_id":1})
            .await
            .unwrap()
            .try_collect()
            .await
            .unwrap();
        assert_eq!(frozen, unchanged);
        store.reauthorize(&old).await.unwrap();
        store.reauthorize(&new).await.unwrap();
        if retained {
            let selection = ViewInput::default().validate(now()).unwrap();
            assert_eq!(
                store
                    .latest(&new.scope, &selection, now())
                    .await
                    .unwrap()
                    .report
                    .report_id,
                new.report_id
            );
            store
                .collection(JOBS)
                .update_one(
                    doc! {"_id":&old.report_id},
                    doc! {"$set":{"retire_after":now() as i64}},
                )
                .await
                .unwrap();
            assert!(matches!(
                store.verify_snapshot(&old).await,
                Err(MetricsError::Expired)
            ));
            assert_eq!(
                store
                    .latest(&new.scope, &selection, now())
                    .await
                    .unwrap()
                    .report
                    .report_id,
                new.report_id
            );
        } else {
            assert!(matches!(
                store
                    .report(&old.scope, &old.report_id, old.expires_at)
                    .await,
                Err(MetricsError::Expired)
            ));
            store
                .report(&new.scope, &new.report_id, now())
                .await
                .unwrap();
        }
        store.db.drop().await.unwrap();
    }
}
