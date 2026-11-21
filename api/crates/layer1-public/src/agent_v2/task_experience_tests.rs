use axum::{
    body::to_bytes,
    http::{Method, StatusCode},
};
use serde_json::json;
use tower::ServiceExt;

#[tokio::test]
async fn task_experience_default_off_is_private_authenticated_policy_not_transport() {
    let fixture = super::fixtures::fixture(false, true).await;
    let response = fixture
        .router
        .oneshot(super::fixtures::request(
            Method::GET,
            "/api/v2/ui/task-experience",
            "owner-a",
            None,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["cache-control"], "private, no-store");
    let body: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(
        body,
        json!({
            "object": "cma.ui.task_experience", "schema_version": 1, "account_id": "owner-a",
            "capabilities": {"task_home_admission": false, "suggested_followup_admission": false,
                "desktop_enable": false, "browser_takeover": false, "replay_presentation": false}
        })
    );
}
