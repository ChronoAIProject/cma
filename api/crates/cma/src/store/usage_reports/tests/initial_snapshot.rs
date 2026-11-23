use super::*;
use layer2_core::{Authorization, Caller, Capability};

async fn ready(retained: bool) -> (Arc<ReportingStore>, Report, ReportLease) {
    let store = if retained {
        super::retained_views::enabled().await
    } else {
        fixture().await
    };
    seed(&store, &[state(0, now() - 3600), state(1, now() - 3600)]).await;
    let principal = scope("operator", true);
    let pending = if retained {
        store
            .latest(
                &principal,
                &ViewInput::default().validate(now()).unwrap(),
                now(),
            )
            .await
            .unwrap()
            .report
    } else {
        store
            .submit(
                principal.clone(),
                ReportInput::default().validate(now()).unwrap(),
                now(),
            )
            .await
            .unwrap()
    };
    let lease = store
        .claim_report("report:0", "initial-worker")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(lease.report.report_id, pending.report_id);
    let generated =
        crate::sandbox_metrics::generate_fixture_with_collection(store.clone(), &lease, false)
            .await
            .unwrap();
    store.finish_report(&lease, generated, true).await.unwrap();
    let report = store
        .report(&principal, &pending.report_id, now())
        .await
        .unwrap();
    (store, report, lease)
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn initial_namespaces_reject_missing_members_and_charts_in_both_report_kinds() {
    for retained in [false, true] {
        let (store, report, _) = ready(retained).await;
        let generation = store.initial_snapshot_generation(&report).await.unwrap();
        assert!(generation > 0);
        for name in [ROWS, CHARTS] {
            let saved = store
                .collection(name)
                .find_one(doc! {"report_id":&report.report_id})
                .await
                .unwrap()
                .unwrap();
            store
                .collection(name)
                .delete_one(doc! {"_id":saved.get("_id").unwrap()})
                .await
                .unwrap();
            assert!(
                matches!(
                    store.initial_snapshot_generation(&report).await,
                    Err(MetricsError::Unavailable)
                ),
                "{name} retained={retained}"
            );
            store.collection(name).insert_one(saved).await.unwrap();
            assert_eq!(
                store.initial_snapshot_generation(&report).await.unwrap(),
                generation
            );
        }
        store.db.drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn initial_namespace_proof_binds_scope_descriptor_state_generation_and_expiry() {
    for retained in [false, true] {
        let (store, report, _) = ready(retained).await;
        let filter = doc! {"_id":&report.report_id};
        let saved = store
            .collection(JOBS)
            .find_one(filter.clone())
            .await
            .unwrap()
            .unwrap();
        for mutation in [
            doc! {"state":"running"},
            doc! {"generation":0_i64},
            doc! {"record.mapping_revision":99_i64},
            doc! {"record.scope.principal":"foreign"},
            doc! {"principal":"foreign"},
            doc! {"admin":false},
            doc! {"row_count":99_i64},
            doc! {"chart_count":99_i64},
            doc! {"lease_until":now() as i64+60},
            doc! {"logical_expires_at":now() as i64-1},
            doc! {"retire_after":now() as i64},
            doc! {"retire_after":"invalid"},
            doc! {"retire_after":mongodb::bson::Bson::Null},
            doc! {"kind":"unknown"},
        ] {
            store
                .collection(JOBS)
                .update_one(filter.clone(), doc! {"$set":&mutation})
                .await
                .unwrap();
            assert!(
                store.initial_snapshot_generation(&report).await.is_err(),
                "{mutation:?}, retained={retained}"
            );
            store
                .collection(JOBS)
                .replace_one(filter.clone(), saved.clone())
                .await
                .unwrap();
        }
        if retained {
            store
                .collection(JOBS)
                .update_one(filter.clone(), doc! {"$unset":{"retire_after":""}})
                .await
                .unwrap();
            assert!(store.initial_snapshot_generation(&report).await.is_err());
            store
                .collection(JOBS)
                .replace_one(filter.clone(), saved.clone())
                .await
                .unwrap();
        }
        let mut altered = report.clone();
        altered.summary.as_mut().unwrap().sandbox_count += 1;
        assert!(store.initial_snapshot_generation(&altered).await.is_err());
        altered = report.clone();
        altered.expires_at = now();
        assert!(matches!(
            store.initial_snapshot_generation(&altered).await,
            Err(MetricsError::Expired)
        ));
        store.collection(JOBS).delete_one(filter).await.unwrap();
        assert!(matches!(
            store.initial_snapshot_generation(&report).await,
            Err(MetricsError::NotFound)
        ));
        store.db.drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn completed_initial_membership_refuses_late_supported_writers() {
    for retained in [false, true] {
        let (store, report, lease) = ready(retained).await;
        let generation = store.initial_snapshot_generation(&report).await.unwrap();
        assert!(store
            .write_report_chunk(&lease, &[fixture_row(&state(2, now() - 3600))], vec![])
            .await
            .is_err());
        assert!(store
            .finish_report(&lease, report.clone(), true)
            .await
            .is_err());
        assert!(store
            .claim_report("report:1", "late-worker")
            .await
            .unwrap()
            .is_none());
        assert_eq!(
            store.initial_snapshot_generation(&report).await.unwrap(),
            generation
        );
        assert_eq!(
            store
                .collection(ROWS)
                .count_documents(doc! {"report_id":&report.report_id})
                .await
                .unwrap(),
            2
        );
        store.db.drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn initial_service_reads_complete_bound_projections_and_rechecks_live_owners() {
    for retained in [false, true] {
        let (store, report, _) = ready(retained).await;
        let service = MetricsService::new(store.clone());
        let caller = Caller::new(
            SubjectId("operator".into()),
            Authorization::new([Capability::ReadAllSandboxes]),
        );
        let projection = InitialProjection {
            first_page: InitialPage {
                limit: 1,
                ..Default::default()
            },
            total_metrics: vec![Metric::Cpu, Metric::Memory, Metric::Disk],
        };
        let result = service
            .initial_report(&caller, true, &report.report_id, projection.clone(), now())
            .await
            .unwrap();
        let initial = result.initial.unwrap();
        assert_eq!(initial.rows.items.len(), 1);
        assert_eq!(initial.rows.total_count, 2);
        assert!(initial.rows.next.is_some());
        assert_eq!(initial.charts.len(), 3);
        assert!(initial
            .charts
            .iter()
            .all(|chart| chart.lines.len() == 1 && chart.lines[0].id == "total"));
        let selected = if retained {
            service
                .latest_initial(
                    &caller,
                    true,
                    InitialViewInput {
                        selection: ViewInput::default(),
                        projection: projection.clone(),
                    },
                    report.as_of,
                )
                .await
                .unwrap()
                .view
                .report
        } else {
            service
                .submit_initial(
                    &caller,
                    true,
                    InitialReportInput {
                        query: ReportInput {
                            from: Some(report.query.from),
                            to: Some(report.query.to),
                            step_seconds: Some(report.query.step_seconds),
                            filters: report.query.filters.clone(),
                        },
                        projection: projection.clone(),
                    },
                    report.as_of,
                )
                .await
                .unwrap()
                .report
        };
        assert_eq!(selected.report_id, report.report_id);
        store
            .collection("sandboxes")
            .insert_one(doc! {"_id":state(1, now()).sandbox_id.as_str(),"owner":"transferred"})
            .await
            .unwrap();
        assert!(matches!(
            service
                .initial_report(&caller, true, &report.report_id, projection, now())
                .await,
            Err(MetricsError::NotFound)
        ));
        store.db.drop().await.unwrap();
    }
}
