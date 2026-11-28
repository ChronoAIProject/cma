use super::*;
use crate::config::report_views::{ViewPolicy, WarmScope};
use crate::store::migrations::usage_projections::{self, COLLECTION};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn projected_reports_preserve_series_owner_pages_and_fail_closed() {
    let old = fixture().await;
    let store = old.with_view_policy(ViewPolicy {
        aggregate_projections_enabled: true,
        view_scopes: vec![WarmScope {
            admin: true,
            principal: "operator".into(),
            sandbox_id: None,
        }],
        ..Default::default()
    });
    old.require_projection_schema().await.unwrap();
    assert!(store.require_projection_schema().await.is_err());
    for (collection, index) in usage_projections::inspect(&store.db).await.unwrap() {
        store
            .db
            .run_command(doc! {"createIndexes":collection,"indexes":[index]})
            .await
            .unwrap();
    }
    store.require_projection_schema().await.unwrap();

    let at = now();
    let states: Vec<_> = (0..24)
        .map(|id| {
            let mut row = state(id, at - 3600);
            row.owner = SubjectId(format!("owner-{:02}", id % 22));
            row
        })
        .collect();
    seed(&store, &states).await;
    store.collection("users").insert_many([
        doc! {"_id":"owner-00","display_name":"Fixture Owner","first_seen_at":"2026-01-01T00:00:00Z"},
        doc! {"_id":"owner-01","display_name":"Fixture Owner","first_seen_at":"2026-02-01T00:00:00Z"},
    ]).await.unwrap();
    let owner = scope("operator", true);
    let pending = store
        .submit(
            owner.clone(),
            ReportInput::default().validate(at).unwrap(),
            at,
        )
        .await
        .unwrap();
    let lease = store
        .claim_report("report:0", "aggregate-fixture")
        .await
        .unwrap()
        .unwrap();
    let generated =
        crate::sandbox_metrics::generate_fixture_with_collection(store.clone(), &lease, false)
            .await
            .unwrap();
    assert_ne!(generated.summary.as_ref().unwrap().sandbox_count, 0);
    store.finish_report(&lease, generated, true).await.unwrap();
    let report = store
        .report(&owner, &pending.report_id, now())
        .await
        .unwrap();
    let generation = store.projected_generation(&report).await.unwrap().unwrap();
    assert_eq!(generation, lease.generation);

    for metric in [Metric::Cpu, Metric::Memory, Metric::Disk] {
        let total = SeriesQuery {
            metric,
            group_by: GroupBy::Total,
            series_id: None,
            top: None,
        };
        let cases = std::iter::once(total.clone())
            .chain(
                [GroupBy::Owner, GroupBy::Sandbox]
                    .into_iter()
                    .flat_map(|group_by| {
                        (1..=20).map(move |top| SeriesQuery {
                            metric,
                            group_by,
                            series_id: None,
                            top: Some(top),
                        })
                    }),
            )
            .chain([
                SeriesQuery {
                    group_by: GroupBy::Owner,
                    series_id: Some("owner-21".into()),
                    ..total.clone()
                },
                SeriesQuery {
                    group_by: GroupBy::Sandbox,
                    series_id: Some(states[23].sandbox_id.to_string()),
                    ..total
                },
            ]);
        for query in cases {
            let legacy = old.series(&report, query.clone()).await.unwrap();
            let cached = store.series(&report, query).await.unwrap();
            assert_eq!(
                serde_json::to_vec(&cached).unwrap(),
                serde_json::to_vec(&legacy).unwrap()
            );
        }
        let mut cursor = None;
        let mut count = 0;
        loop {
            let query = OwnerAllocationQuery {
                metric,
                limit: Some(7),
                cursor,
            };
            let legacy = old
                .owner_allocations(&report, query.clone(), OwnerContextAccess::DateOnly)
                .await
                .unwrap();
            let cached = store
                .owner_allocations(&report, query, OwnerContextAccess::DateOnly)
                .await
                .unwrap();
            assert_eq!(
                serde_json::to_value(&cached).unwrap(),
                serde_json::to_value(&legacy).unwrap()
            );
            count += cached.items.len();
            cursor = cached.next;
            if cursor.is_none() {
                break;
            }
        }
        assert_eq!(count, 22);
    }

    let prefix = format!("/api/v1/admin/usage/metrics/reports/{}", report.report_id);
    for suffix in ["/series?metric=cpu", "/owner-allocations?metric=cpu"] {
        let path = format!("{prefix}{suffix}");
        for (caller, admin, expected) in [
            ("operator", true, 200),
            ("another-operator", true, 404),
            ("operator", false, 403),
        ] {
            assert_eq!(
                super::allocated::request(
                    &store,
                    "GET",
                    &path,
                    caller,
                    admin,
                    serde_json::Value::Null,
                )
                .await
                .0,
                expected,
            );
        }
    }
    store
        .collection("sandboxes")
        .insert_one(doc! {
            "_id":states[0].sandbox_id.as_str(),"owner":"changed-owner"
        })
        .await
        .unwrap();
    for suffix in ["/series?metric=cpu", "/owner-allocations?metric=cpu"] {
        assert_eq!(
            super::allocated::request(
                &store,
                "GET",
                &format!("{prefix}{suffix}"),
                "operator",
                true,
                serde_json::Value::Null,
            )
            .await
            .0,
            404,
        );
    }
    store
        .collection("sandboxes")
        .delete_one(doc! {
            "_id":states[0].sandbox_id.as_str()
        })
        .await
        .unwrap();

    let removed = store
        .collection(COLLECTION)
        .delete_many(doc! {
            "report_id":&report.report_id,"kind":"chart_total","metric":Metric::Cpu.index() as i64,
        })
        .await
        .unwrap();
    assert!(removed.deleted_count > 0);
    let total = SeriesQuery {
        metric: Metric::Cpu,
        group_by: GroupBy::Total,
        series_id: None,
        top: None,
    };
    assert!(matches!(
        store.series(&report, total.clone()).await,
        Err(MetricsError::Unavailable)
    ));
    assert!(old.series(&report, total).await.is_ok());
    store.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn projection_parent_survives_delayed_pruning_for_ready_and_failed_reports() {
    let store = fixture().await;
    for (collection, index) in usage_projections::inspect(&store.db).await.unwrap() {
        store
            .db
            .run_command(doc! {"createIndexes":collection,"indexes":[index]})
            .await
            .unwrap();
    }
    for success in [true, false] {
        let at = now();
        let principal = if success {
            "ready-operator"
        } else {
            "failed-operator"
        };
        store
            .submit(
                scope(principal, true),
                ReportInput::default().validate(at).unwrap(),
                at,
            )
            .await
            .unwrap();
        let lease = store
            .claim_report("report:0", principal)
            .await
            .unwrap()
            .unwrap();
        let mut batch = cma_am_reporting::ProjectionBatch::new(32_768);
        for (kind, series, pages) in [
            ("chart_total", "", 64_i64),
            ("chart_sandbox", "fixture-sandbox", 1),
            ("contributor", "fixture-owner", 1),
        ] {
            for chunk in 0..pages {
                batch
                    .push(
                        doc! {"report_id":&lease.report.report_id,"generation":lease.generation,
                        "document_schema_version":1_i64,"kind":kind,"metric":0_i64,
                        "series":series,"chunk":chunk},
                    )
                    .unwrap();
            }
        }
        let documents = batch.into_documents();
        let bytes = documents
            .iter()
            .map(|row| row.as_bytes().len() as i64)
            .sum();
        assert!(store
            .persist_projections(&lease, &documents, bytes, (0, 0))
            .await
            .unwrap());
        store
            .finish_report(&lease, lease.report.clone(), success)
            .await
            .unwrap();

        let filter = doc! {"_id":&lease.report.report_id};
        let job = store
            .collection(JOBS)
            .find_one(filter.clone())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(job.get("expires_at"), Some(&mongodb::bson::Bson::Null));
        let manifest = job.get_document("projection_manifest").unwrap();
        let manifest_bytes = mongodb::bson::serialize_to_vec(
            &doc! {"projection_generation":lease.generation,"projection_manifest":manifest},
        )
        .unwrap()
        .len()
            - 5;
        assert_eq!(
            job.get_i64("reserved_bytes").unwrap(),
            bytes + manifest_bytes as i64
        );
        assert_eq!(manifest.get_i64("page_bytes").unwrap(), bytes);
        // The original TTL would have passed while the pruner was unavailable.
        store
            .collection(JOBS)
            .update_one(
                filter.clone(),
                doc! {"$set":{"logical_expires_at":at.saturating_sub(86_401) as i64}},
            )
            .await
            .unwrap();
        store.prune_cache(now()).await.unwrap();
        let held = store
            .collection(JOBS)
            .find_one(filter.clone())
            .await
            .unwrap()
            .unwrap();
        assert!(held.get_bool("reserved").unwrap());
        assert_eq!(held.get("expires_at"), Some(&mongodb::bson::Bson::Null));
        assert_eq!(
            store
                .collection(COLLECTION)
                .count_documents(doc! {"report_id":&lease.report.report_id,"kind":"contributor"})
                .await
                .unwrap(),
            1
        );
        store.prune_cache(now()).await.unwrap();
        let released = store
            .collection(JOBS)
            .find_one(filter)
            .await
            .unwrap()
            .unwrap();
        assert!(!released.get_bool("reserved").unwrap());
        assert_eq!(released.get_i64("bytes").unwrap(), 0);
        assert!(released.get_datetime("expires_at").is_ok());
        assert_eq!(
            store
                .collection(COLLECTION)
                .count_documents(doc! {"report_id":&lease.report.report_id})
                .await
                .unwrap(),
            0
        );
    }
    store.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn account_erasure_invalidates_aggregate_before_first_commit() {
    use crate::store::account_erasure::{erase_records, Cursor, Target};
    use layer2_core::account_deletion::ErasureInventory;

    let store = fixture().await;
    for (collection, index) in usage_projections::inspect(&store.db).await.unwrap() {
        store
            .db
            .run_command(doc! {"createIndexes":collection,"indexes":[index]})
            .await
            .unwrap();
    }
    let at = now();
    store
        .submit(
            scope("operator", true),
            ReportInput::default().validate(at).unwrap(),
            at,
        )
        .await
        .unwrap();
    let lease = store
        .claim_report("report:0", "aggregate-worker")
        .await
        .unwrap()
        .unwrap();
    let rows = [
        fixture_row(&state(0, at - 3600)),
        fixture_row(&state(1, at - 3600)),
    ];
    let mut charts = Vec::new();
    for row in &rows {
        for metric in [Metric::Cpu, Metric::Memory, Metric::Disk] {
            charts.extend(
                chart_documents(
                    &lease.report,
                    &row.sandbox_id,
                    &row.owner.subject,
                    metric,
                    Vec::new(),
                    &[],
                )
                .unwrap(),
            );
        }
    }
    store
        .write_prepared_report_chunk(&lease, &rows, charts)
        .await
        .unwrap();
    let mut report = lease.report.clone();
    report.summary = Some(Summary {
        sandbox_count: 2,
        ..Default::default()
    });
    let (batch, row_count, chart_count) = store.build_projections(&lease, &report).await.unwrap();
    let documents = batch.into_documents();
    let bytes = documents
        .iter()
        .map(|row| row.as_bytes().len() as i64)
        .sum();
    assert_eq!((row_count, chart_count), (2, 6));
    assert_eq!(
        store
            .collection(COLLECTION)
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );

    let owner = SubjectId::new(rows[0].owner.subject.clone());
    let inventory = ErasureInventory::default();
    let mut cursor = Cursor::default();
    let mut removed = false;
    for _ in 0..700 {
        let batch = erase_records(
            &store.db,
            &[Target {
                subject: &owner,
                inventory: &inventory,
            }],
            &mut cursor,
            &layer2_core::now_rfc3339(),
        )
        .await
        .unwrap();
        if [ROWS, CHARTS]
            .iter()
            .any(|name| batch.deleted.get(*name).copied().unwrap_or_default() > 0)
        {
            removed = true;
            break;
        }
        if !batch.has_more {
            break;
        }
    }
    assert!(removed);
    let job = store
        .collection(JOBS)
        .find_one(doc! {"_id":&report.report_id})
        .await
        .unwrap()
        .unwrap();
    assert!(job.get_bool(INVALIDATED_FIELD).unwrap());
    let original_bytes = job.get_i64("bytes").unwrap();
    assert!(matches!(
        store
            .persist_projections(&lease, &documents, bytes, (row_count, chart_count))
            .await,
        Err(MetricsError::Unavailable)
    ));
    assert_eq!(
        store
            .collection(COLLECTION)
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    let job = store
        .collection(JOBS)
        .find_one(doc! {"_id":&report.report_id})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(job.get_i64("bytes").unwrap(), original_bytes);
    assert!(job.get_datetime("expires_at").is_ok());
    store.db.drop().await.unwrap();
}
