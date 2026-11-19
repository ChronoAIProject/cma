use super::*;

#[tokio::test]
async fn agent_list_pagination_keeps_typed_refusals_and_private_headers() {
    let fixture = fixture().await;
    for query in [
        "limit=-1",
        "limit=0",
        "limit=201",
        "limit=65536",
        "limit=abc",
        "unknown=private-input",
    ] {
        let response = fixture
            .router
            .clone()
            .oneshot(request(
                Method::GET,
                &format!("/api/v1/agents?{query}"),
                OWNER,
                None,
                None,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "application/problem+json"
        );
        assert_eq!(
            response.headers()[header::CACHE_CONTROL],
            "private, no-store"
        );
        let body: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 16384).await.unwrap()).unwrap();
        assert_eq!(body["code"], "invalid_instance_query");
        assert_eq!(
            body["detail"],
            "The instance query parameters are not valid."
        );
    }
    let mut anonymous = request(Method::GET, "/api/v1/agents?limit=-1", OWNER, None, None);
    anonymous.extensions_mut().remove::<AuthenticatedSubject>();
    assert_eq!(
        fixture.router.oneshot(anonymous).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn agent_list_preserves_two_hundred_default_and_owner_scope() {
    let fixture = fixture().await;
    for index in 0..201 {
        let agent = seeded_agent(&format!("agt_page_{index:03}"), AgentStatus::Ready, 2, None);
        insert_agent(&fixture.store, agent, vec![]).await;
    }
    for (suffix, count) in [("", 200), ("?limit=1", 1), ("?limit=200", 200)] {
        let (status, body) = json_response(
            &fixture.router,
            request(
                Method::GET,
                &format!("/api/v1/agents{suffix}"),
                OWNER,
                None,
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["items"].as_array().unwrap().len(), count);
        assert!(body["next"].is_string());
    }
    let (status, body) = json_response(
        &fixture.router,
        request(Method::GET, "/api/v1/agents?limit=200", OTHER, None, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["items"].as_array().unwrap().is_empty());
}
