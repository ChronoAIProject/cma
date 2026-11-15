use super::allocated::request;
use super::retained_views::enabled;
use super::*;
use serde_json::Value;

async fn reopen(store: &ReportingStore) -> Arc<ReportingStore> {
    ReportingStore::connect(
        &std::env::var("CHRONO_TEST_MONGODB_URI").unwrap(),
        store.db.name(),
        &[19; 32],
    )
    .await
    .expect("retained pointers must satisfy the normal startup schema gate")
}

async fn complete(store: &Arc<ReportingStore>, lease: &ReportLease) {
    let generated =
        crate::sandbox_metrics::generate_fixture_with_collection(store.clone(), lease, false)
            .await
            .unwrap();
    store.finish_report(lease, generated, true).await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn retained_pointer_restarts_preserve_pending_ready_and_running_replacement() {
    let store = enabled().await;
    seed(&store, &[state(0, now() - 86_400)]).await;
    let principal = scope("operator", true);
    let mut selection = ViewInput::default().validate(now()).unwrap();
    let pending = store.latest(&principal, &selection, now()).await.unwrap();
    let first_id = pending.report.report_id.clone();
    let key = selection.key(&principal).unwrap();
    let pointer = store
        .collection(CONTROL)
        .find_one(doc! {"_id":&key})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        pointer.get_i64("document_schema_version").unwrap(),
        super::super::super::schema::DOCUMENT_SCHEMA_VERSION
    );

    // Reopen through the production initializer, including its unknown-version
    // scan. Disabling writers must not prevent recovery of existing namespaces.
    let disabled = reopen(&store).await;
    assert!(matches!(
        disabled.latest(&principal, &selection, now()).await,
        Err(MetricsError::ViewDisabled)
    ));
    let reopened = disabled.with_view_policy((*store.view_policy).clone());
    let observed = reopened
        .latest(&principal, &selection, now())
        .await
        .unwrap();
    assert_eq!(observed.report.report_id, first_id);
    assert_eq!(observed.report.status, ReportStatus::Pending);
    let lease = reopened
        .claim_report("report:0", "restart-initial")
        .await
        .unwrap()
        .unwrap();
    complete(&reopened, &lease).await;

    let disabled = reopen(&store).await;
    let path = format!("/api/v1/admin/usage/metrics/reports/{first_id}");
    let (status, body) = request(&disabled, "GET", &path, "operator", true, Value::Null).await;
    assert_eq!(status, 200);
    assert_eq!(body["status"], "ready");
    let page = request(
        &disabled,
        "GET",
        &format!("{path}/sandboxes?limit=50"),
        "operator",
        true,
        Value::Null,
    )
    .await;
    assert_eq!(page.0, 200);
    assert_eq!(page.1["items"].as_array().unwrap().len(), 1);
    let reopened = disabled.with_view_policy((*store.view_policy).clone());
    let ready = reopened
        .latest(&principal, &selection, now())
        .await
        .unwrap();
    assert_eq!(ready.report.report_id, first_id);
    assert_eq!(ready.refresh.state, RefreshState::Idle);

    selection.refresh = true;
    let refreshing = reopened
        .latest(&principal, &selection, now())
        .await
        .unwrap();
    let replacement_id = refreshing.refresh.report_id.unwrap();
    assert_ne!(replacement_id, first_id);
    assert_eq!(refreshing.report.report_id, first_id);
    let replacement = reopened
        .claim_report("report:0", "restart-replacement")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(replacement.report.report_id, replacement_id);

    let recovered = reopen(&store)
        .await
        .with_view_policy((*store.view_policy).clone());
    selection.refresh = false;
    let observed = recovered
        .latest(&principal, &selection, now())
        .await
        .unwrap();
    assert_eq!(observed.report.report_id, first_id);
    assert_eq!(
        observed.refresh.report_id.as_deref(),
        Some(replacement_id.as_str())
    );
    assert_eq!(observed.refresh.state, RefreshState::Pending);
    complete(&recovered, &replacement).await;

    let finished = reopen(&store)
        .await
        .with_view_policy((*store.view_policy).clone());
    let observed = finished
        .latest(&principal, &selection, now())
        .await
        .unwrap();
    assert_eq!(observed.report.report_id, replacement_id);
    assert_eq!(observed.report.status, ReportStatus::Ready);
    assert_eq!(observed.refresh.state, RefreshState::Idle);
    assert_eq!(observed.report.summary.unwrap().sandbox_count, 1);
    store.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn retained_pointer_restart_still_refuses_missing_or_unknown_schema() {
    let store = enabled().await;
    let principal = scope("operator", true);
    let selection = ViewInput::default().validate(now()).unwrap();
    store.latest(&principal, &selection, now()).await.unwrap();
    let filter = doc! {"_id":selection.key(&principal).unwrap()};
    for mutation in [
        doc! {"$unset":{"document_schema_version":""}},
        doc! {"$set":{"document_schema_version":99_i64}},
        doc! {"$set":{"document_schema_version":"1"}},
    ] {
        store
            .collection(CONTROL)
            .update_one(filter.clone(), mutation)
            .await
            .unwrap();
        let before = store
            .collection(CONTROL)
            .find_one(filter.clone())
            .await
            .unwrap();
        let result = ReportingStore::connect(
            &std::env::var("CHRONO_TEST_MONGODB_URI").unwrap(),
            store.db.name(),
            &[19; 32],
        )
        .await;
        assert!(matches!(result, Err(MetricsError::Unavailable)));
        let after = store
            .collection(CONTROL)
            .find_one(filter.clone())
            .await
            .unwrap();
        assert_eq!(before, after, "startup must not rewrite invalid pointers");
    }
    store.db.drop().await.unwrap();
}
