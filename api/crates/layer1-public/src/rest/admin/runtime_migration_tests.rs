use super::*;

#[tokio::test]
async fn migration_and_observation_capabilities_are_independent_and_default_closed() {
    let f = fixture(false);
    let path = "/api/v1/admin/sandboxes/sbx_update/runtime-migration";
    for caps in [
        vec![],
        vec![Capability::UseOwnSandboxes],
        vec![Capability::ReadAllSandboxes],
    ] {
        let response = send(
            &f.app,
            Method::POST,
            path,
            Some(json!({"clear":true})),
            caps,
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
    let response = send(
        &f.app,
        Method::POST,
        path,
        Some(json!({"quiet_since":"2026-01-01T00:00:00Z"})),
        [Capability::ControlAnySandbox],
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let observation = "/api/v1/admin/sandboxes/sbx_update/runtime-update/activity";
    let response = send(
        &f.app,
        Method::GET,
        observation,
        None,
        [Capability::ControlAnySandbox],
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let response = send(
        &f.app,
        Method::GET,
        observation,
        None,
        [Capability::ReadAllSandboxes],
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(
        body,
        json!({"status":"unknown","reason":"unqualified_source"})
    );
    assert!(f.activity.records.lock().unwrap().is_empty());
}
