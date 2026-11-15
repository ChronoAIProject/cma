//! Warm selections bind their rolling range while retaining the subject cohort.
use super::*;
use crate::config::report_views::{ViewPolicy, WarmScope, WarmSelection};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn retained_warmer_publishes_three_distinct_ranges_and_exact_ids_stay_terminal() {
    let original = fixture().await;
    let mut configured = (*original).clone();
    // Reduce reservations to exercise all three jobs without widening budgets.
    configured.budgets.report_bytes = 16 * 1024 * 1024;
    let store = configured.with_view_policy(ViewPolicy {
        enabled: true,
        warm_enabled: true,
        view_scopes: vec![WarmScope {
            principal: "fixture-admin".into(),
            admin: true,
            sandbox_id: None,
        }],
        warm_scopes: [ViewWindow::Day, ViewWindow::Week, ViewWindow::Month]
            .map(|window| WarmSelection {
                principal: "fixture-admin".into(),
                admin: true,
                sandbox_id: None,
                window,
            })
            .to_vec(),
        ..Default::default()
    });
    seed(&store, &[state(0, now() - 31 * 86400)]).await;
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
        3
    );
    for _ in 0..3 {
        let lease = store
            .claim_report("report:0", "window-test")
            .await
            .unwrap()
            .unwrap();
        let report =
            crate::sandbox_metrics::generate_fixture_with_collection(store.clone(), &lease, false)
                .await
                .unwrap();
        store.finish_report(&lease, report, true).await.unwrap();
    }
    let principal = scope("fixture-admin", true);
    let mut ids = std::collections::BTreeSet::new();
    for window in [ViewWindow::Day, ViewWindow::Week, ViewWindow::Month] {
        let selection = ViewInput {
            window,
            ..Default::default()
        }
        .validate(now())
        .unwrap();
        let view = store.latest(&principal, &selection, now()).await.unwrap();
        assert_eq!(view.window, window);
        assert_eq!(
            view.report.query.to - view.report.query.from,
            window.seconds().unwrap()
        );
        assert!(view.generated_at.is_some());
        assert_eq!(view.refresh.state, RefreshState::Idle);
        assert!(ids.insert(view.report.report_id));
    }
    let mut selection = ViewInput::default().validate(now()).unwrap();
    selection.refresh = true;
    let replacement = store.latest(&principal, &selection, now()).await.unwrap();
    let failed_id = replacement.refresh.report_id.unwrap();
    let lease = store
        .claim_report("report:0", "window-failure")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(lease.report.report_id, failed_id);
    store
        .finish_report(&lease, lease.report.clone(), false)
        .await
        .unwrap();
    for _ in 0..3 {
        assert!(matches!(
            store.report(&principal, &failed_id, now()).await,
            Err(MetricsError::Expired)
        ));
    }
    assert_eq!(
        store
            .collection(JOBS)
            .count_documents(doc! {})
            .await
            .unwrap(),
        4
    );
    selection.refresh = false;
    let retained = store.latest(&principal, &selection, now()).await.unwrap();
    assert_eq!(retained.report.report_id, replacement.report.report_id);
    assert!(retained.generated_at.is_some());
    assert!(matches!(
        store
            .latest(&scope("different-admin", true), &selection, now())
            .await,
        Err(MetricsError::ViewDisabled)
    ));
    store.db.drop().await.unwrap();
}
