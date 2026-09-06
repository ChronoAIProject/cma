use super::*;

#[tokio::test]
async fn current_defaults_are_owner_visible_and_expose_only_pinned_references() {
    let (router, settings) = fixture_with_settings(false);
    let make_request = || {
        let mut request = request(
            Method::GET,
            "/api/v1/defaults",
            None,
            Some(("owner", false)),
            None,
        );
        request
            .extensions_mut()
            .insert(Authorization::new([Capability::UseOwnSandboxes]));
        request
    };
    let response = router.clone().oneshot(make_request()).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
    let defaults = settings.0.lock().unwrap().settings.defaults.clone();
    assert_eq!(
        body,
        json!({
            "sandbox_profile": {"id": defaults.sandbox_profile.id, "revision": defaults.sandbox_profile.revision},
            "workspace_profile": {"id": defaults.workspace_profile.id, "revision": defaults.workspace_profile.revision},
            "agent_profile": {"id": defaults.agent_profile.id, "revision": defaults.agent_profile.revision}
        })
    );
    settings
        .0
        .lock()
        .unwrap()
        .settings
        .defaults
        .agent_profile
        .revision += 1;
    let response = router.oneshot(make_request()).await.unwrap();
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
    assert_eq!(
        body["agent_profile"]["revision"],
        defaults.agent_profile.revision + 1
    );
}

#[tokio::test]
async fn defaults_require_owner_capability_not_profile_management_or_visitor_access() {
    let router = fixture(false);
    let response = router
        .clone()
        .oneshot(request(Method::GET, "/api/v1/defaults", None, None, None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    for capabilities in [
        vec![Capability::ManageOwnProfiles],
        vec![Capability::LaunchTriggers],
        vec![],
    ] {
        let mut request = request(
            Method::GET,
            "/api/v1/defaults",
            None,
            Some(("visitor", false)),
            None,
        );
        request
            .extensions_mut()
            .insert(Authorization::new(capabilities));
        let response = router.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
}
