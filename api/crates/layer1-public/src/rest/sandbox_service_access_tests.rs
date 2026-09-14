use super::*;

const PATH: &str = "/api/v1/sandboxes/sbx_access/service-access";

#[tokio::test]
async fn service_access_is_owner_only_and_idempotent_without_rewriting_profile() {
    let fixture = fixture(3);
    let original = paused_sandbox("sbx_access", "owner");
    fixture
        .store
        .create_sandbox(original.clone())
        .await
        .unwrap();
    let body = json!({"services": ["api-github"]});
    for (subject, expected) in [
        (None, StatusCode::UNAUTHORIZED),
        (
            Some(("owner-a", Authorization::new([]))),
            StatusCode::FORBIDDEN,
        ),
        (
            Some(("other-owner", all_capabilities())),
            StatusCode::NOT_FOUND,
        ),
    ] {
        let (status, _) = json_response(
            &fixture.router,
            request(
                Method::POST,
                PATH,
                Some(body.clone()),
                subject,
                Some("access-guard"),
            ),
        )
        .await;
        assert_eq!(status, expected);
    }
    let (status, result) = json_response(
        &fixture.router,
        authenticated(Method::POST, PATH, Some(body.clone()), Some("grant-access")),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(result["nyxid_services"], json!(["api-github"]));
    assert_eq!(result["status"], "paused");
    assert!(result.get("credential").is_none());
    let granted = fixture
        .store
        .get_sandbox(&original.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(granted.profile, original.profile);
    assert_eq!(granted.credential.reaches, original.credential.reaches);
    assert_eq!(granted.credential.service_grants.len(), 1);
    let replay = json_response(
        &fixture.router,
        authenticated(Method::POST, PATH, Some(body.clone()), Some("grant-access")),
    )
    .await;
    assert_eq!(replay, (status, result));
    let mut live = granted.clone();
    live.status = SandboxStatus::Active;
    live.revision += 1;
    fixture
        .store
        .save_sandbox(granted.revision, live.clone())
        .await
        .unwrap();
    let replay = json_response(
        &fixture.router,
        authenticated(Method::POST, PATH, Some(body), Some("replay-live")),
    )
    .await;
    assert_eq!(replay.0, StatusCode::OK);
    assert_eq!(replay.1["status"], "active");
    assert_eq!(
        fixture.store.get_sandbox(&original.id).await.unwrap(),
        Some(live)
    );
    assert_eq!(
        fixture
            .store
            .sandbox_events(&original.id, 0, 20)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn service_access_refuses_invalid_bodies_and_live_expansion_without_mutation() {
    let fixture = fixture(3);
    let original = paused_sandbox("sbx_access", "owner");
    fixture
        .store
        .create_sandbox(original.clone())
        .await
        .unwrap();
    for (index, body) in [
        json!({"services": []}),
        json!({"services": [""]}),
        json!({"services": ["bad\nservice"]}),
        json!({"services": ["x".repeat(257)]}),
    ]
    .into_iter()
    .enumerate()
    {
        let (status, result) = json_response(
            &fixture.router,
            authenticated(
                Method::POST,
                PATH,
                Some(body),
                Some(&format!("invalid-access-{index}")),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(result["code"], "invalid_sandbox_service_access");
    }
    let mut active = original.clone();
    active.status = SandboxStatus::Active;
    active.revision += 1;
    fixture
        .store
        .save_sandbox(original.revision, active.clone())
        .await
        .unwrap();
    let (status, result) = json_response(
        &fixture.router,
        authenticated(
            Method::POST,
            PATH,
            Some(json!({"services": ["api-github"]})),
            Some("live-access"),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(result["code"], "sandbox_service_access_requires_pause");
    assert_eq!(
        fixture.store.get_sandbox(&original.id).await.unwrap(),
        Some(active)
    );
    assert!(fixture
        .store
        .sandbox_events(&original.id, 0, 20)
        .await
        .unwrap()
        .is_empty());
}
