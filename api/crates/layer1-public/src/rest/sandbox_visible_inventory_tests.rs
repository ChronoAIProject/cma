use super::*;

#[tokio::test]
async fn visible_inventory_query_keeps_history_optional_and_filter_before_pagination() {
    let fixture = fixture(10);
    for (id, status) in [
        ("sbx_00", SandboxStatus::Retired),
        ("sbx_01", SandboxStatus::Active),
        ("sbx_02", SandboxStatus::Retired),
        ("sbx_03", SandboxStatus::Retiring),
    ] {
        fixture
            .store
            .create_sandbox(stored_sandbox(id, "owner", status, 4))
            .await
            .unwrap();
    }
    for (query, expected, next) in [
        (
            "?exclude_retired=true&limit=1",
            vec!["sbx_01"],
            Some("sbx_01"),
        ),
        (
            "?exclude_retired=true&limit=1&cursor=sbx_01",
            vec!["sbx_03"],
            None,
        ),
        ("?exclude_retired=true&status=retired", vec![], None),
        (
            "?exclude_retired=false",
            vec!["sbx_00", "sbx_01", "sbx_02", "sbx_03"],
            None,
        ),
        ("", vec!["sbx_00", "sbx_01", "sbx_02", "sbx_03"], None),
    ] {
        let (status, body) = json_response(
            &fixture.router,
            authenticated(
                Method::GET,
                &format!("/api/v1/sandboxes{query}"),
                None,
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let ids: Vec<_> = body["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, expected);
        assert_eq!(body["next"].as_str(), next);
    }
    let (status, _) = json_response(
        &fixture.router,
        authenticated(
            Method::GET,
            "/api/v1/sandboxes?exclude_retired=invalid",
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}
