use super::allocated::request;
use super::*;
use crate::config::report_views::{ViewPolicy, WarmScope};
use std::time::Instant;

pub(super) async fn enabled() -> Arc<ReportingStore> {
    fixture().await.with_view_policy(ViewPolicy {
        enabled: true,
        view_scopes: ["operator", "foreign"]
            .map(|principal| WarmScope {
                principal: principal.into(),
                admin: true,
                sandbox_id: None,
            })
            .to_vec(),
        ..Default::default()
    })
}
pub(super) async fn build(
    store: &Arc<ReportingStore>,
    principal: &Scope,
    selection: &ViewSelection,
) -> Report {
    let pending = store.latest(principal, selection, now()).await.unwrap();
    let id = pending.refresh.report_id.as_ref().unwrap();
    let lease = store
        .claim_report("report:0", "retained-worker")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&lease.report.report_id, id);
    let generated =
        crate::sandbox_metrics::generate_fixture_with_collection(store.clone(), &lease, false)
            .await
            .unwrap();
    store.finish_report(&lease, generated, true).await.unwrap();
    store.report(principal, id, now()).await.unwrap()
}
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn retained_views_coalesce_refresh_and_preserve_last_good_on_failure_and_pressure() {
    let store = enabled().await;
    seed(&store, &[state(0, now() - 86_400)]).await;
    let principal = scope("operator", true);
    let mut selection = ViewInput::default().validate(now()).unwrap();
    let original = build(&store, &principal, &selection).await;
    assert!(original.expires_at >= now() + 86_390);
    let ready = store.latest(&principal, &selection, now()).await.unwrap();
    assert_eq!(ready.report.report_id, original.report_id);
    assert!(!ready.stale);
    store.reauthorize(&ready.report).await.unwrap();
    let child = store
        .collection(ROWS)
        .find_one(doc! {"report_id":&original.report_id})
        .await
        .unwrap()
        .unwrap();
    assert!(
        child.get_datetime("expires_at").unwrap().timestamp_millis()
            >= (original.expires_at + 30) as i64 * 1000
    );
    selection.refresh = true;
    let (a, b) = tokio::join!(
        store.latest(&principal, &selection, now()),
        store.latest(&principal, &selection, now())
    );
    let (a, b) = (a.unwrap(), b.unwrap());
    assert_eq!(a.refresh.report_id, b.refresh.report_id);
    assert_eq!(a.report.report_id, original.report_id);
    assert_eq!(a.refresh.state, RefreshState::Pending);
    selection.refresh = false;
    let observed = store.latest(&principal, &selection, now()).await.unwrap();
    assert_eq!(observed.report.report_id, original.report_id);
    assert_eq!(observed.refresh.state, RefreshState::Pending);
    assert_eq!(observed.refresh.report_id, a.refresh.report_id);
    // Predecessor workers cannot claim or rewrite retained namespaces.
    assert_eq!(
        store
            .collection(JOBS)
            .count_documents(doc! {"state":{"$in":["pending","running"]}})
            .await
            .unwrap(),
        0
    );
    let lease = store
        .claim_report("report:0", "failed-refresh")
        .await
        .unwrap()
        .unwrap();
    store
        .finish_report(&lease, lease.report.clone(), false)
        .await
        .unwrap();
    selection.refresh = false;
    let unchanged = store.latest(&principal, &selection, now()).await.unwrap();
    assert_eq!(unchanged.report.report_id, original.report_id);
    let mut pressured = (*store).clone();
    pressured.budgets.global_bytes = pressured.budgets.report_bytes;
    pressured.budgets.tenant_bytes = pressured.budgets.report_bytes;
    selection.refresh = true;
    let deferred = pressured
        .latest(&principal, &selection, now())
        .await
        .unwrap();
    assert_eq!(deferred.report.report_id, original.report_id);
    assert_eq!(deferred.refresh.state, RefreshState::Deferred);
    // Freshness is independent of selection identity and replacement success.
    selection.refresh = false;
    let stale_at = unchanged.generated_at.unwrap() + store.view_policy.freshness_seconds;
    let stale = store
        .latest(&principal, &selection, stale_at)
        .await
        .unwrap();
    assert!(stale.stale);
    assert_eq!(stale.report.report_id, original.report_id);
    assert_eq!(stale.refresh.state, RefreshState::Pending);
    store.db.drop().await.unwrap();
}
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn retained_view_reads_reject_partial_children_live_revocation_and_expiry() {
    let store = enabled().await;
    seed(&store, &[state(0, now() - 86_400)]).await;
    let principal = scope("operator", true);
    let selection = ViewInput::default().validate(now()).unwrap();
    let report = build(&store, &principal, &selection).await;
    store.reauthorize(&report).await.unwrap();
    store
        .collection("sandbox_usage")
        .update_one(
            doc! {"_id":format!("sbx_{:032x}",0)},
            doc! {"$set":{"owner":"transferred"}},
        )
        .await
        .unwrap();
    assert!(store.reauthorize(&report).await.is_err());
    store
        .collection("sandbox_usage")
        .update_one(
            doc! {"_id":format!("sbx_{:032x}",0)},
            doc! {"$set":{"owner":"owner-0"}},
        )
        .await
        .unwrap();
    store.reauthorize(&report).await.unwrap();
    let page = store.page(&report, PageQuery::default()).await.unwrap();
    assert_eq!(page.items.len(), 1);
    // A cleanup between authorization/child read and response verification
    // cannot turn missing membership into permission for residual charts.
    store
        .collection(ROWS)
        .delete_many(doc! {"report_id":&report.report_id})
        .await
        .unwrap();
    assert!(matches!(
        store.verify_snapshot(&report).await,
        Err(MetricsError::Unavailable)
    ));
    assert!(matches!(
        store.reauthorize(&report).await,
        Err(MetricsError::Unavailable)
    ));
    store
        .collection(JOBS)
        .update_one(
            doc! {"_id":&report.report_id},
            doc! {"$set":{"retire_after":now() as i64}},
        )
        .await
        .unwrap();
    assert!(matches!(
        store.verify_snapshot(&report).await,
        Err(MetricsError::Expired)
    ));
    store.prune_cache(now()).await.unwrap();
    assert_eq!(
        store
            .collection(CHARTS)
            .count_documents(doc! {"report_id":&report.report_id})
            .await
            .unwrap(),
        0
    );
    assert!(!store
        .collection(JOBS)
        .find_one(doc! {"_id":&report.report_id})
        .await
        .unwrap()
        .unwrap()
        .get_bool("reserved")
        .unwrap());
    store.db.drop().await.unwrap();
}
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn retained_view_empty_reports_disabled_writer_and_foreign_pointer_fail_closed() {
    let store = fixture().await;
    let principal = scope("operator", true);
    let selection = ViewInput::default().validate(now()).unwrap();
    assert!(matches!(
        store.latest(&principal, &selection, now()).await,
        Err(MetricsError::ViewDisabled)
    ));
    assert_eq!(
        store
            .collection(JOBS)
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    let store = store.with_view_policy(ViewPolicy {
        enabled: true,
        view_scopes: ["operator", "foreign"]
            .map(|principal| WarmScope {
                principal: principal.into(),
                admin: true,
                sandbox_id: None,
            })
            .to_vec(),
        ..Default::default()
    });
    let report = build(&store, &principal, &selection).await;
    assert_eq!(report.summary.as_ref().unwrap().sandbox_count, 0);
    store.reauthorize(&report).await.unwrap();
    let foreign = scope("foreign", true);
    let foreign_report = build(&store, &foreign, &selection).await;
    store
        .collection(CONTROL)
        .update_one(
            doc! {"_id":selection.key(&principal).unwrap()},
            doc! {"$set":{"latest_id":foreign_report.report_id}},
        )
        .await
        .unwrap();
    assert!(store.latest(&principal, &selection, now()).await.is_err());
    store.db.drop().await.unwrap();
}
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn retained_view_replacement_fences_late_worker_and_preserves_old_read_grace() {
    let store = enabled().await;
    seed(&store, &[state(0, now() - 86_400)]).await;
    let principal = scope("operator", true);
    let mut selection = ViewInput::default().validate(now()).unwrap();
    let original = build(&store, &principal, &selection).await;
    selection.refresh = true;
    let pending = store.latest(&principal, &selection, now()).await.unwrap();
    let old = store
        .claim_report("report:0", "late-worker")
        .await
        .unwrap()
        .unwrap();
    store
        .collection(JOBS)
        .update_one(
            doc! {"_id":&old.report.report_id},
            doc! {"$set":{"lease_until":0_i64}},
        )
        .await
        .unwrap();
    store.release("report:0", "late-worker").await.unwrap();
    let new = store
        .claim_report("report:0", "current-worker")
        .await
        .unwrap()
        .unwrap();
    assert!(new.generation > old.generation);
    let generated =
        crate::sandbox_metrics::generate_fixture_with_collection(store.clone(), &new, false)
            .await
            .unwrap();
    store.finish_report(&new, generated, true).await.unwrap();
    assert!(store
        .finish_report(&old, old.report.clone(), true)
        .await
        .is_err());
    selection.refresh = false;
    let committed = store.latest(&principal, &selection, now()).await.unwrap();
    assert_eq!(
        Some(&committed.report.report_id),
        pending.refresh.report_id.as_ref()
    );
    store.reauthorize(&original).await.unwrap();
    store
        .collection(JOBS)
        .update_one(
            doc! {"_id":&original.report_id},
            doc! {"$set":{"retire_after":now() as i64}},
        )
        .await
        .unwrap();
    store.prune_cache(now()).await.unwrap();
    store.reauthorize(&committed.report).await.unwrap();
    assert!(store.reauthorize(&original).await.is_err());
    let started = Instant::now();
    let (status, body) = request(
        &store,
        "GET",
        "/api/v1/admin/usage/metrics/view?window=24h",
        "operator",
        true,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, 200);
    let bytes = serde_json::to_vec(&body).unwrap().len();
    assert!(bytes < 1024 * 1024);
    eprintln!(
        "retained_small response_us={} bytes={bytes}",
        started.elapsed().as_micros()
    );
    store.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn retained_warmer_is_off_by_default_and_enforces_explicit_owned_canary_scope() {
    let store = fixture().await;
    crate::sandbox_metrics::warm_fixture(store.clone())
        .await
        .unwrap();
    assert_eq!(
        store
            .collection(JOBS)
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    assert!(store
        .collection(CONTROL)
        .find_one(doc! {"_id":"view-warmer"})
        .await
        .unwrap()
        .is_none());
    seed(&store, &[state(0, now() - 86_400)]).await;
    let canary = WarmScope {
        principal: "owner-0".into(),
        admin: true,
        sandbox_id: Some(format!("sbx_{:032x}", 0)),
    };
    let store = store.with_view_policy(ViewPolicy {
        enabled: true,
        warm_enabled: true,
        view_scopes: vec![canary.clone()],
        warm_scopes: vec![canary.into()],
        ..Default::default()
    });
    let default = ViewInput::default().validate(now()).unwrap();

    crate::sandbox_metrics::warm_fixture(store.clone())
        .await
        .unwrap();
    crate::sandbox_metrics::warm_fixture(store.clone())
        .await
        .unwrap();
    assert_eq!(
        store
            .collection(JOBS)
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    let job = store
        .collection(JOBS)
        .find_one(doc! {})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(job.get("expires_at"), Some(&mongodb::bson::Bson::Null));
    store
        .collection("sandbox_usage")
        .update_one(
            doc! {"_id":format!("sbx_{:032x}",0)},
            doc! {"$set":{"owner":"foreign"}},
        )
        .await
        .unwrap();
    assert!(crate::sandbox_metrics::warm_fixture(store.clone())
        .await
        .is_err());
    assert_eq!(
        store
            .collection(JOBS)
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    let unlisted = store
        .latest(&scope("owner-0", true), &default, now())
        .await
        .unwrap();
    let ordinary = store
        .collection(JOBS)
        .find_one(doc! {"_id":&unlisted.report.report_id})
        .await
        .unwrap()
        .unwrap();
    assert!(!ordinary.contains_key("kind"));
    assert_eq!(ordinary.get("expires_at"), Some(&mongodb::bson::Bson::Null));
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
