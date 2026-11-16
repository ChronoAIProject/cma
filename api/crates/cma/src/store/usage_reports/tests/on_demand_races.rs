use super::{
    allocated::request,
    on_demand_views::{enabled, finish},
    *,
};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn unlisted_concurrent_admission_and_refresh_coalesce_and_preserve_old_identity() {
    let store = enabled().await;
    let remote = ReportingStore::connect(
        &std::env::var("CHRONO_TEST_MONGODB_URI").unwrap(),
        store.db.name(),
        &[19; 32],
    )
    .await
    .unwrap()
    .with_view_policy((*store.view_policy).clone());
    let principal = scope("owner", false);
    let mut selection = ViewInput::default().validate(now()).unwrap();
    let at = now();
    let (a, b) = tokio::join!(
        store.latest(&principal, &selection, at),
        remote.latest(&principal, &selection, at)
    );
    let (a, b) = (a.unwrap(), b.unwrap());
    assert_eq!(a.report.report_id, b.report.report_id);
    let original = finish(&store, &a.report.report_id, true).await;
    selection.refresh = true;
    let (a, b) = tokio::join!(
        store.latest(&principal, &selection, at),
        remote.latest(&principal, &selection, at)
    );
    let (a, b) = (a.unwrap(), b.unwrap());
    assert_eq!(a.report.report_id, b.report.report_id);
    assert_ne!(a.report.report_id, original.report_id);
    let retained = store
        .report(&principal, &original.report_id, now())
        .await
        .unwrap();
    assert_eq!(encode(&retained).unwrap(), encode(&original).unwrap());
    assert_eq!(
        store
            .collection(JOBS)
            .count_documents(doc! {})
            .await
            .unwrap(),
        2
    );
    let old = store
        .collection(JOBS)
        .find_one(doc! {"_id":&original.report_id})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        old.get_str("query_hash").unwrap(),
        format!("replaced-selection:{}", original.report_id)
    );
    finish(&remote, &a.report.report_id, true).await;
    selection.refresh = false;
    assert_eq!(
        store
            .latest(&principal, &selection, now())
            .await
            .unwrap()
            .report
            .report_id,
        a.report.report_id
    );
    store.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn unlisted_refresh_preserves_running_worker_and_stale_rotation_cannot_erase_completion() {
    let store = enabled().await;
    let principal = scope("owner", false);
    let mut selection = ViewInput::default().validate(now()).unwrap();
    let pending = store.latest(&principal, &selection, now()).await.unwrap();
    let previous = store
        .collection(JOBS)
        .find_one(doc! {"_id":&pending.report.report_id})
        .await
        .unwrap()
        .unwrap();
    let key = previous.get_str("query_hash").unwrap();
    let lease = store
        .claim_report("report:0", "racing-worker")
        .await
        .unwrap()
        .unwrap();
    selection.refresh = true;
    let observed = store.latest(&principal, &selection, now()).await.unwrap();
    assert_eq!(observed.report.report_id, pending.report.report_id);
    let generated =
        crate::sandbox_metrics::generate_fixture_with_collection(store.clone(), &lease, false)
            .await
            .unwrap();
    let (finished, refresh) = tokio::join!(
        store.finish_report(&lease, generated, true),
        store.latest(&principal, &selection, now())
    );
    finished.unwrap();
    let refresh = refresh.unwrap();
    assert!(
        store
            .report(&principal, &pending.report.report_id, now())
            .await
            .unwrap()
            .report_complete
    );
    assert!(matches!(
        store
            .admit_new(
                principal.clone(),
                selection.query.clone(),
                now(),
                key.into(),
                Some(&previous)
            )
            .await,
        Err(MetricsError::Unavailable)
    ));
    let count = store
        .collection(JOBS)
        .count_documents(doc! {})
        .await
        .unwrap();
    assert_eq!(
        count,
        if refresh.report.report_id == pending.report.report_id {
            1
        } else {
            2
        }
    );
    selection.refresh = false;
    let next = store.latest(&principal, &selection, now()).await.unwrap();
    assert_eq!(next.report.report_id, refresh.report.report_id);
    store.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn unlisted_failed_reads_stay_terminal_until_explicit_refresh() {
    let store = enabled().await;
    let principal = scope("owner", false);
    let mut selection = ViewInput::default().validate(now()).unwrap();
    let first = store.latest(&principal, &selection, now()).await.unwrap();
    finish(&store, &first.report.report_id, false).await;
    assert!(matches!(
        store.latest(&principal, &selection, now()).await,
        Err(MetricsError::Unavailable)
    ));
    assert_eq!(
        store
            .collection(JOBS)
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    selection.refresh = true;
    let replacement = store.latest(&principal, &selection, now()).await.unwrap();
    assert_ne!(replacement.report.report_id, first.report.report_id);
    finish(&store, &replacement.report.report_id, true).await;
    let (status, _) = request(
        &store,
        "GET",
        &format!("/api/v1/usage/metrics/reports/{}", first.report.report_id),
        "owner",
        false,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, 503);
    selection.refresh = false;
    assert_eq!(
        store
            .latest(&principal, &selection, now())
            .await
            .unwrap()
            .report
            .report_id,
        replacement.report.report_id
    );
    store.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn unlisted_expiry_replaces_selection_without_extending_old_report_ttl() {
    let store = enabled().await;
    let principal = scope("owner", false);
    let selection = ViewInput::default().validate(now()).unwrap();
    let pending = store.latest(&principal, &selection, now()).await.unwrap();
    let original = finish(&store, &pending.report.report_id, true).await;
    let at = original.expires_at;
    let current = ViewInput::default().validate(at).unwrap();
    let replacement = store.latest(&principal, &current, at).await.unwrap();
    assert_ne!(replacement.report.report_id, original.report_id);
    assert_eq!(replacement.report.query, current.query);
    assert!(matches!(
        store.report(&principal, &original.report_id, at).await,
        Err(MetricsError::Expired)
    ));
    assert_eq!(
        store
            .collection(JOBS)
            .count_documents(doc! {})
            .await
            .unwrap(),
        2
    );
    store.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn unlisted_budget_rejection_rolls_back_rotation_and_circuit_still_precedes_fallback() {
    let store = enabled().await;
    let principal = scope("owner", false);
    let mut selection = ViewInput::default().validate(now()).unwrap();
    let pending = store.latest(&principal, &selection, now()).await.unwrap();
    let original = finish(&store, &pending.report.report_id, true).await;
    let before = store
        .collection(JOBS)
        .find_one(doc! {"_id":&original.report_id})
        .await
        .unwrap()
        .unwrap();
    let mut pressured = (*store).clone();
    pressured.budgets.global_bytes = 0;
    selection.refresh = true;
    assert!(matches!(
        pressured.latest(&principal, &selection, now()).await,
        Err(MetricsError::Busy)
    ));
    let after = store
        .collection(JOBS)
        .find_one(doc! {"_id":&original.report_id})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(before, after);
    selection.refresh = false;
    assert_eq!(
        store
            .latest(&principal, &selection, now())
            .await
            .unwrap()
            .report
            .report_id,
        original.report_id
    );
    store
        .circuit
        .store(now() + 30, std::sync::atomic::Ordering::Relaxed);
    let (status, _) = request(
        &store,
        "GET",
        "/api/v1/usage/metrics/view",
        "different",
        false,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, 429);
    assert_eq!(
        store
            .collection(JOBS)
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    store.db.drop().await.unwrap();
}
