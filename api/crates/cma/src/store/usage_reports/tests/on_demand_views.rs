use super::{allocated::request, *};
use crate::config::report_views::{ViewPolicy, WarmScope};

pub(super) async fn enabled() -> Arc<ReportingStore> {
    fixture().await.with_view_policy(ViewPolicy {
        enabled: true,
        view_scopes: [false, true]
            .map(|admin| WarmScope {
                principal: "listed".into(),
                admin,
                sandbox_id: None,
            })
            .to_vec(),
        ..Default::default()
    })
}

pub(super) async fn finish(store: &Arc<ReportingStore>, id: &str, success: bool) -> Report {
    let lease = store
        .claim_report("report:0", "on-demand-worker")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(lease.report.report_id, id);
    assert!(lease.view.is_none());
    let report = if success {
        crate::sandbox_metrics::generate_fixture_with_collection(store.clone(), &lease, false)
            .await
            .unwrap()
    } else {
        lease.report.clone()
    };
    store.finish_report(&lease, report, success).await.unwrap();
    let job = store
        .collection(JOBS)
        .find_one(doc! {"_id":id})
        .await
        .unwrap()
        .unwrap();
    decode(&job).unwrap()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn unlisted_routes_complete_personal_admin_and_all_three_windows() {
    let store = enabled().await;
    seed(
        &store,
        &[state(0, now() - 31 * 86400), state(1, now() - 31 * 86400)],
    )
    .await;
    for admin in [false, true] {
        let base = if admin {
            "/api/v1/admin/usage/metrics"
        } else {
            "/api/v1/usage/metrics"
        };
        for window in ["24h", "7d", "30d"] {
            let path = format!("{base}/view?window={window}");
            let (status, pending) = request(
                &store,
                "GET",
                &path,
                "owner-0",
                admin,
                serde_json::Value::Null,
            )
            .await;
            assert_eq!(status, 202, "{pending:?}");
            let id = pending["report"]["report_id"].as_str().unwrap();
            assert_eq!(pending["refresh"]["report_id"], id);
            assert!(pending["generated_at"].is_null());
            let before = now();
            let finished = finish(&store, id, true).await;
            let completed = finished.expires_at - REPORT_TTL_SECONDS;
            assert!((before..=now()).contains(&completed));
            let (status, descriptor) = request(
                &store,
                "GET",
                &format!("{base}/reports/{id}"),
                "owner-0",
                admin,
                serde_json::Value::Null,
            )
            .await;
            assert_eq!(status, 200);
            assert_eq!(descriptor["report_id"], id);
            let (status, ready) = request(
                &store,
                "GET",
                &path,
                "owner-0",
                admin,
                serde_json::Value::Null,
            )
            .await;
            assert_eq!(status, 200, "{ready:?}");
            assert_eq!(ready["report"]["report_id"], id);
            assert_eq!(ready["report"]["scope"]["admin"], admin);
            assert_eq!(
                ready["report"]["summary"]["sandbox_count"],
                if admin { 2 } else { 1 }
            );
            assert_eq!(ready["generated_at"], completed);
            assert_eq!(ready["refresh"]["state"], "idle");
            assert_eq!(ready["stale"], false);
            let job = store
                .collection(JOBS)
                .find_one(doc! {"_id":id})
                .await
                .unwrap()
                .unwrap();
            assert!(!job.contains_key("kind"));
            assert_eq!(job.get("expires_at"), Some(&mongodb::bson::Bson::Null));
            assert!(matches!(
                store.report(&scope("foreign", admin), id, now()).await,
                Err(MetricsError::NotFound)
            ));
        }
    }
    crate::sandbox_metrics::warm_fixture(store.clone())
        .await
        .unwrap();
    assert_eq!(
        store
            .collection(JOBS)
            .count_documents(doc! {})
            .await
            .unwrap(),
        6
    );
    assert_eq!(
        store
            .collection(CONTROL)
            .count_documents(doc! {"kind":super::super::views::VIEW_KIND})
            .await
            .unwrap(),
        0
    );
    store.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn views_off_stays_disabled_and_listed_personal_admin_keep_retained_jobs() {
    let original = fixture().await;
    for admin in [false, true] {
        let path = if admin {
            "/api/v1/admin/usage/metrics/view"
        } else {
            "/api/v1/usage/metrics/view"
        };
        for owner in ["listed", "unlisted"] {
            let (status, body) = request(
                &original,
                "GET",
                path,
                owner,
                admin,
                serde_json::Value::Null,
            )
            .await;
            assert_eq!(status, 501);
            assert_eq!(body["code"], "report_view_disabled");
        }
    }
    assert_eq!(
        original
            .collection(JOBS)
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    original.db.drop().await.unwrap();
    let store = enabled().await;
    let selection = ViewInput::default().validate(now()).unwrap();
    for admin in [false, true] {
        let principal = scope("listed", admin);
        let report = super::retained_views::build(&store, &principal, &selection).await;
        let view = store.latest(&principal, &selection, now()).await.unwrap();
        assert_eq!(view.report.report_id, report.report_id);
        assert!(view.generated_at.is_some());
        assert!(report.expires_at > now() + REPORT_TTL_SECONDS);
        let job = store
            .collection(JOBS)
            .find_one(doc! {"_id":&report.report_id})
            .await
            .unwrap()
            .unwrap();
        assert_eq!(job.get_str("kind").unwrap(), super::super::views::VIEW_KIND);
    }
    store.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn unlisted_pending_and_completed_identity_survive_minute_rollover_and_replica_change() {
    let store = enabled().await;
    let principal = scope("owner", false);
    let before = now() - 61;
    let selection = ViewInput::default().validate(before).unwrap();
    let pending = store.latest(&principal, &selection, before).await.unwrap();
    let remote = ReportingStore::connect(
        &std::env::var("CHRONO_TEST_MONGODB_URI").unwrap(),
        store.db.name(),
        &[19; 32],
    )
    .await
    .unwrap()
    .with_view_policy((*store.view_policy).clone());
    let current = ViewInput::default().validate(now()).unwrap();
    assert_ne!(current.query.to, selection.query.to);
    let observed = remote.latest(&principal, &current, now()).await.unwrap();
    assert_eq!(observed.report.report_id, pending.report.report_id);
    assert_eq!(observed.refresh.report_id, pending.refresh.report_id);
    finish(&remote, &pending.report.report_id, true).await;
    let ready = store.latest(&principal, &current, now()).await.unwrap();
    assert_eq!(ready.report.status, ReportStatus::Ready);
    assert_eq!(ready.report.report_id, pending.report.report_id);
    assert_eq!(ready.requested_range, current.query.range());
    assert_eq!(
        store
            .collection(JOBS)
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    // No polling-time generation can make admission look like completion.
    assert!(ready.generated_at.unwrap() >= ready.report.as_of);
    let stale = store
        .latest(
            &principal,
            &current,
            ready.generated_at.unwrap() + store.view_policy.freshness_seconds,
        )
        .await
        .unwrap();
    assert!(stale.stale);
    assert_eq!(stale.report.report_id, ready.report.report_id);
    store.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn unlisted_scope_selection_and_live_revocation_are_enforced_before_success() {
    let store = enabled().await;
    seed(&store, &[state(0, now() - 86400), state(1, now() - 86400)]).await;
    let own = format!("sbx_{:032x}", 0);
    let foreign = format!("sbx_{:032x}", 1);
    let base = "/api/v1/usage/metrics/view";
    let selected = |key: &str, value: &str| {
        let filters = serde_json::json!({key: value}).to_string();
        let query = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("filters", &filters)
            .finish();
        format!("{base}?{query}")
    };
    for (path, admin, expected) in [
        ("/api/v1/admin/usage/metrics/view".into(), false, 403),
        (selected("sandbox_id", &foreign), false, 404),
        (selected("owner", "foreign"), false, 400),
        (
            selected("agent_id", &format!("agt_{:032x}", 99)),
            false,
            404,
        ),
    ] {
        assert_eq!(
            request(
                &store,
                "GET",
                &path,
                "owner-0",
                admin,
                serde_json::Value::Null
            )
            .await
            .0,
            expected,
            "{path}"
        );
    }
    assert_eq!(
        store
            .collection(JOBS)
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    let path = selected("sandbox_id", &own);
    let (status, pending) = request(
        &store,
        "GET",
        &path,
        "owner-0",
        false,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, 202);
    finish(
        &store,
        pending["report"]["report_id"].as_str().unwrap(),
        true,
    )
    .await;
    assert_eq!(
        request(
            &store,
            "GET",
            &path,
            "owner-0",
            false,
            serde_json::Value::Null
        )
        .await
        .0,
        200
    );
    store
        .collection("sandboxes")
        .insert_one(doc! {"_id":&own,"owner":"transferred"})
        .await
        .unwrap();
    assert_eq!(
        request(
            &store,
            "GET",
            &path,
            "owner-0",
            false,
            serde_json::Value::Null
        )
        .await
        .0,
        404
    );
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
