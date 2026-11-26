use super::*;

#[tokio::test]
#[ignore = "requires disposable Mongo contract runner with failpoints"]
async fn aggregate_projection_reconciles_committed_manifest_after_lost_acknowledgement() {
    use crate::store::migrations::usage_projections::{self, COLLECTION};
    use cma_am_reporting::ProjectionBatch;

    let f = Fixture::new().await;
    for (collection, index) in usage_projections::inspect(&f.store.db).await.unwrap() {
        f.store
            .db
            .run_command(doc! {"createIndexes":collection,"indexes":[index]})
            .await
            .unwrap();
    }
    let at = now();
    f.store
        .submit(
            scope("operator", true),
            ReportInput::default().validate(at).unwrap(),
            at,
        )
        .await
        .unwrap();
    let lease = f
        .store
        .claim_report("report:0", "aggregation-worker")
        .await
        .unwrap()
        .unwrap();
    let build = |order: [i64; 2], gain: i64| {
        let mut batch = ProjectionBatch::new(32_768);
        for chunk in order {
            batch
                .push(
                    doc! {"report_id":&lease.report.report_id,"generation":lease.generation,
                    "kind":"chart_total","metric":0_i64,"series":"","chunk":chunk,"gain":gain},
                )
                .unwrap();
        }
        let docs = batch.into_documents();
        let bytes = docs.iter().map(|row| row.as_bytes().len() as i64).sum();
        (docs, bytes)
    };
    let (initial, bytes) = build([0, 1], 7);
    f.commands.clear();
    f.fail(&["commitTransaction"], 20, doc! {
        "writeConcernError":{"code":50,"codeName":"MaxTimeMSExpired","errmsg":"injected commit uncertainty"}
    }).await;
    let outcome = f
        .store
        .persist_projections(&lease, &initial, bytes, (0, 0))
        .await;
    f.disable().await;
    assert!(outcome.unwrap());
    assert_eq!(f.commands.attempts(), 1);
    assert_eq!(f.commands.count("commitTransaction"), 5);

    let (replay, replay_bytes) = build([1, 0], 7);
    assert!(f
        .store
        .persist_projections(&lease, &replay, replay_bytes, (0, 0))
        .await
        .unwrap());
    let (amended, amended_bytes) = build([1, 0], 8);
    assert!(matches!(
        f.store
            .persist_projections(&lease, &amended, amended_bytes, (0, 0))
            .await,
        Err(MetricsError::Unavailable)
    ));
    let job = f
        .store
        .collection(JOBS)
        .find_one(doc! {"_id":&lease.report.report_id})
        .await
        .unwrap()
        .unwrap();
    let manifest = job.get_document("projection_manifest").unwrap();
    let manifest_bytes = mongodb::bson::serialize_to_vec(
        &doc! {"projection_generation":lease.generation,"projection_manifest":manifest},
    )
    .unwrap()
    .len()
        - 5;
    assert_eq!(job.get_i64("bytes").unwrap(), bytes + manifest_bytes as i64);
    assert_eq!(manifest.get_i64("page_bytes").unwrap(), bytes);
    let persisted = f
        .store
        .collection(COLLECTION)
        .count_documents(doc! {"report_id":&lease.report.report_id})
        .await
        .unwrap();
    assert_eq!(persisted, 2);
    f.store.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable Mongo contract runner with failpoints"]
async fn optional_chart_query_timeout_retains_completed_totals() {
    use crate::config::report_views::{ViewPolicy, WarmScope};
    use crate::store::migrations::usage_projections;

    let f = Fixture::new().await;
    let store = f.store.with_view_policy(ViewPolicy {
        aggregate_projections_enabled: true,
        view_scopes: vec![WarmScope {
            admin: true,
            principal: "operator".into(),
            sandbox_id: None,
        }],
        ..Default::default()
    });
    for (collection, index) in usage_projections::inspect(&store.db).await.unwrap() {
        store
            .db
            .run_command(doc! {"createIndexes":collection,"indexes":[index]})
            .await
            .unwrap();
    }
    let at = now();
    seed(&store, &[state(0, at - 3600), state(1, at - 3600)]).await;
    let scope = scope("operator", true);
    store
        .submit(
            scope.clone(),
            ReportInput::default().validate(at).unwrap(),
            at,
        )
        .await
        .unwrap();
    let lease = store
        .claim_report("report:0", "optional-family")
        .await
        .unwrap()
        .unwrap();
    // The already accumulated totals require no chart find. The initial
    // optional ranked-family cursor is the first matching read.
    f.fail(
        &["find"],
        1,
        doc! {"namespace":format!("{}.{CHARTS}", store.db.name()), "errorCode":50},
    )
    .await;
    let generated =
        crate::sandbox_metrics::generate_fixture_with_collection(store.clone(), &lease, false)
            .await;
    f.disable().await;
    store
        .finish_report(&lease, generated.unwrap(), true)
        .await
        .unwrap();
    let report = store
        .report(&scope, &lease.report.report_id, now())
        .await
        .unwrap();
    assert_eq!(report.status, ReportStatus::Ready);
    assert!(store
        .projected_family(&report, "chart_total")
        .await
        .unwrap()
        .is_some());
    assert!(store
        .projected_family(&report, "chart_owner")
        .await
        .unwrap()
        .is_none());
    assert!(store
        .projected_family(&report, "chart_sandbox")
        .await
        .unwrap()
        .is_none());
    for group_by in [GroupBy::Sandbox, GroupBy::Owner] {
        let ranked = SeriesQuery {
            metric: Metric::Cpu,
            group_by,
            series_id: None,
            top: Some(1),
        };
        assert!(matches!(
            store.series(&report, ranked.clone()).await,
            Err(MetricsError::Busy)
        ));
        f.store.series(&report, ranked).await.unwrap();
    }
    let query = SeriesQuery {
        metric: Metric::Cpu,
        group_by: GroupBy::Total,
        series_id: None,
        top: None,
    };
    let legacy = f.store.series(&report, query.clone()).await.unwrap();
    let cached = store.series(&report, query).await.unwrap();
    assert_eq!(
        serde_json::to_vec(&legacy).unwrap(),
        serde_json::to_vec(&cached).unwrap()
    );
    store.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable Mongo contract runner with failpoints"]
async fn optional_contributor_query_timeout_preserves_legacy_report() {
    use crate::config::report_views::{ViewPolicy, WarmScope};
    use crate::store::migrations::usage_projections::{self, COLLECTION};

    let f = Fixture::new().await;
    let store = f.store.with_view_policy(ViewPolicy {
        aggregate_projections_enabled: true,
        view_scopes: vec![WarmScope {
            admin: true,
            principal: "operator".into(),
            sandbox_id: None,
        }],
        ..Default::default()
    });
    for (collection, index) in usage_projections::inspect(&store.db).await.unwrap() {
        store
            .db
            .run_command(doc! {"createIndexes":collection,"indexes":[index]})
            .await
            .unwrap();
    }
    let at = now();
    seed(&store, &[state(0, at - 3600), state(1, at - 3600)]).await;
    let scope = scope("operator", true);
    store
        .submit(
            scope.clone(),
            ReportInput::default().validate(at).unwrap(),
            at,
        )
        .await
        .unwrap();
    let lease = store
        .claim_report("report:0", "contributor-family")
        .await
        .unwrap()
        .unwrap();
    let generated =
        crate::sandbox_metrics::generate_fixture_with_collection(f.store.clone(), &lease, false)
            .await
            .unwrap();
    f.commands.clear();
    // The first matching aggregation counts immutable source rows. The next
    // groups those rows by contributor for optional projection construction.
    f.successor.client.database("admin").run_command(doc! {
        "configureFailPoint":"failCommand","mode":{"skip":1},"data":{
            "failCommands":["aggregate"],"appName":&f.app,"namespace":format!("{}.{ROWS}", store.db.name()),"errorCode":50
        }
    }).await.unwrap();
    let outcome = store
        .prepare_aggregate_projections(
            &lease,
            &generated,
            None,
            std::time::Instant::now() + Duration::from_secs(30),
        )
        .await;
    f.disable().await;
    outcome.unwrap();
    let row_aggregations: Vec<_> = f
        .commands
        .started
        .lock()
        .unwrap()
        .iter()
        .filter(|(name, command)| {
            name == "aggregate" && command.get_str("aggregate").ok() == Some(ROWS)
        })
        .map(|(_, command)| command.clone())
        .collect();
    assert_eq!(row_aggregations.len(), 2);
    assert!(row_aggregations[1]
        .get_array("pipeline")
        .unwrap()
        .iter()
        .any(|stage| stage
            .as_document()
            .and_then(|doc| doc.get_document("$group").ok())
            .is_some_and(|group| group.get_str("_id").ok() == Some("$owner"))));
    store.finish_report(&lease, generated, true).await.unwrap();
    let report = store
        .report(&scope, &lease.report.report_id, now())
        .await
        .unwrap();
    assert_eq!(report.status, ReportStatus::Ready);
    assert!(store
        .projected_family(&report, "chart_total")
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        store
            .collection(COLLECTION)
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    let query = SeriesQuery {
        metric: Metric::Cpu,
        group_by: GroupBy::Total,
        series_id: None,
        top: None,
    };
    let legacy = f.store.series(&report, query.clone()).await.unwrap();
    let fallback = store.series(&report, query).await.unwrap();
    assert_eq!(
        serde_json::to_vec(&legacy).unwrap(),
        serde_json::to_vec(&fallback).unwrap()
    );
    store.db.drop().await.unwrap();
}
