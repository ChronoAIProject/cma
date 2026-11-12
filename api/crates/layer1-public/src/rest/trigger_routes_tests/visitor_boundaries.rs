use axum::http::{header, Method, StatusCode};
use layer2_core::trigger_domain::{LaunchPageRequest, LaunchStore};
use layer2_core::{Caller, SubjectId};

use super::support_tests::*;

#[tokio::test]
async fn anonymous_launch_is_refused_before_any_effect() {
    let app = TestApp::new().await;
    let publisher = Caller::new(SubjectId::new(OWNER), publisher_authority());
    // Seed the public target without creating request idempotency records.
    let trigger = app
        .source_service
        .create(
            &publisher,
            serde_json::from_value(trigger_body("always_new")["content"].clone()).unwrap(),
        )
        .await
        .unwrap();
    let published = app
        .source_service
        .publish(&publisher, &trigger.id, trigger.draft_version)
        .await
        .unwrap();
    let path = format!("/api/v1/triggers/public/{}/launches", published.public_id);

    for _ in 0..2 {
        let (status, headers, bytes) = raw_response(
            &app.router,
            request(
                Method::POST,
                &path,
                Some(launch_body(true)),
                None,
                Some("anonymous-launch"),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(headers[header::CONTENT_TYPE], "application/problem+json");
        let problem: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(problem["code"], "not_authenticated");
        for owner in [VISITOR, OWNER] {
            let launches = app
                .launches
                .list_launches(&SubjectId::new(owner), &LaunchPageRequest::default())
                .await
                .unwrap();
            assert_eq!(launches.items.len(), 0);
        }
        let statistics = app
            .source_service
            .statistics(&publisher, &trigger.id)
            .await
            .unwrap();
        assert_eq!(statistics.totals.launches_total, 0);
        assert_eq!(statistics.totals.agents_ready_total, 0);
        // No reservation means the initially empty idempotency store has no records.
        assert_eq!(app.operation_reservations(), 0);
        assert_eq!(app.quota.reserve_calls(), 0);
        assert_eq!(app.broker.create_calls(), 0);
        assert!(app.quota.settlements().is_empty());
    }
}
