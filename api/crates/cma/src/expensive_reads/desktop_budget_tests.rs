use super::*;

#[tokio::test]
async fn busy_read_capacity_is_distinct_from_a_caller_budget_and_does_not_block_desktop() {
    let budget = budget(2, 1024);
    let held = budget.slots.clone().try_acquire_many_owned(2).unwrap();
    let router = protected(
        Router::new()
            .route("/api/v1/agents/{id}/usage", get(|| async { "usage" }))
            .route(
                "/api/v1/agents/{id}/transcript",
                get(|| async { "history" }),
            )
            .route("/api/v1/agents/{id}", get(|| async { "detail" }))
            .route("/api/v1/sandboxes/{id}", get(|| async { "preparation" }))
            .route("/api/v1/agents/{id}/desktop", get(|| async { "discovery" }))
            .route(
                "/api/v1/agents/{id}/desktop/ws",
                get(|| async { "upgrade" }),
            ),
        budget.clone(),
    );
    for path in ["usage", "transcript"] {
        let response = request(&router, Method::GET, &format!("/api/v1/agents/a/{path}")).await;
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(response.headers()[header::RETRY_AFTER], "1");
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let value: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 1024).await.unwrap()).unwrap();
        assert_eq!(value["code"], "buffered_read_capacity_exhausted");
        assert!(!value["detail"].as_str().unwrap().contains("caller"));
    }
    for path in [
        "/api/v1/agents/a",
        "/api/v1/sandboxes/s",
        "/api/v1/agents/a/desktop",
        "/api/v1/agents/a/desktop/ws",
    ] {
        assert_eq!(
            request(&router, Method::GET, path).await.status(),
            StatusCode::OK
        );
    }
    drop(held);
    let response = request(&router, Method::GET, "/api/v1/agents/a/transcript").await;
    assert_eq!(response.status(), StatusCode::OK);
    drop(response);
    assert_eq!(budget.slots.available_permits(), 2);
}
