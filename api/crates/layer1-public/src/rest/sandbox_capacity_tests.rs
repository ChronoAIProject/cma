use super::*;

#[tokio::test]
async fn capacity_counts_all_own_non_retired_parents_without_agents() {
    let f = fixture(3);
    for status in [
        SandboxStatus::Active,
        SandboxStatus::Paused,
        SandboxStatus::Failed,
        SandboxStatus::Rotating,
        SandboxStatus::Retiring,
    ] {
        f.store
            .create_sandbox(stored_sandbox(status.name(), "owner", status, 4))
            .await
            .unwrap();
    }
    let mut retired = stored_sandbox("retired", "owner", SandboxStatus::Retired, 4);
    retired.retired_at = Some(NOW.into());
    f.store.create_sandbox(retired).await.unwrap();
    f.store
        .create_sandbox(stored_sandbox("other", "other", SandboxStatus::Active, 4))
        .await
        .unwrap();
    let (status, body) = json_response(
        &f.router,
        authenticated(
            Method::GET,
            "/api/v1/sandboxes/capacity?owner=other",
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["resource"], "sandbox");
    assert_eq!(body["limit"], 3);
    assert_eq!(body["live"], 5);
    assert_eq!(body["available"], 0);
    assert_eq!(body["retiring_released"], 0);
    assert_eq!(
        body["counts_by_status"],
        json!({"active":1,"paused":1,"failed":1,"rotating":1,"retiring":1})
    );
    assert!(body["observed_at"].as_str().unwrap().contains('T'));
    let (status, body) = json_response(
        &f.router,
        request(
            Method::GET,
            "/api/v1/sandboxes/capacity",
            None,
            Some(("other", all_capabilities())),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["live"], 1);
    assert_eq!(body["available"], 2);
}

#[tokio::test]
async fn capacity_requires_authenticated_owner_capability() {
    let f = fixture(10);
    for (subject, expected) in [
        (None, StatusCode::UNAUTHORIZED),
        (
            Some(("owner", Authorization::new([]))),
            StatusCode::FORBIDDEN,
        ),
    ] {
        let (status, _) = json_response(
            &f.router,
            request(
                Method::GET,
                "/api/v1/sandboxes/capacity",
                None,
                subject,
                None,
            ),
        )
        .await;
        assert_eq!(status, expected);
    }
}

#[tokio::test]
async fn lifecycle_quota_refusal_reuses_the_sandbox_problem_shape() {
    use axum::response::IntoResponse;
    let response = crate::rest::AgentLifecycleProvisioningError::SandboxLimitReached {
        limit: 10,
        live: 12,
        quota_usage: Some(layer2_core::sandbox::SandboxQuotaUsage {
            active: 8,
            retiring_charged: 4,
            retiring_released: 1,
        }),
    }
    .into_response();
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(body["code"], "sandbox_limit_reached");
    assert_eq!(body["limit"], 10);
    assert_eq!(body["live"], 12);
    assert_eq!(
        body["quota_usage"],
        json!({"active":8,"retiring_charged":4,"retiring_released":1})
    );
}
