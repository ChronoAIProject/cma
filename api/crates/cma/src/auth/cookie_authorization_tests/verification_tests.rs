use super::*;

#[tokio::test]
async fn cookie_uses_live_reduced_permissions_on_every_request_without_writes() {
    let fixture = Fixture::new().await;
    let state = fixture.seed().await;
    let first = fixture.request(state.clone()).await.unwrap();
    assert_eq!(
        first.authorization,
        Authorization::new([Capability::UseOwnSandboxes])
    );
    assert_eq!(first.profile.display_name.as_deref(), Some("Live Owner"));
    assert!(first.delegated_credential.is_some());
    fixture.assert_unchanged(&state).await;

    *fixture.control.introspection.write().await =
        Reply::json(json!({"active": true, "permissions": []}));
    let second = fixture.request(state.clone()).await.unwrap();
    assert_eq!(second.authorization, Authorization::none());
    assert!(second.delegated_credential.is_some());
    assert_eq!(fixture.control.requests.load(Ordering::SeqCst), 5);
    fixture.assert_unchanged(&state).await;
}

#[tokio::test]
async fn inactive_cookie_rejects_browser_and_retained_authority() {
    let fixture = Fixture::new().await;
    let state = fixture.seed().await;
    *fixture.control.introspection.write().await = Reply::json(json!({"active": false}));
    assert_unavailable(fixture.request(state.clone()).await);
    assert!(
        request_for_browser_cookie(&fixture.auth, &state.subject, state.clone())
            .await
            .unwrap()
            .is_none()
    );
    fixture.assert_unchanged(&state).await;

    let metadata = fixture.auth.metadata().await.unwrap();
    assert!(
        identity::resolve_subject(&fixture.auth, &metadata, "access")
            .await
            .is_err()
    );
    assert!(identity::request_for_bearer(&fixture.auth, "access".into())
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn relay_owner_projection_cannot_establish_ordinary_bearer_or_login_authority() {
    use base64::Engine as _;
    let fixture = Fixture::new().await;
    let metadata = fixture.auth.metadata().await.unwrap();
    let before = fixture.control.requests.load(Ordering::SeqCst);
    for channel in [
        json!({"relay":true, "sub":"owner", "permissions":["cma:*"]}),
        json!({"token_type":"relay_callback"}),
        json!({"token_type":"relay_reply"}),
    ] {
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&channel).unwrap());
        let token = format!("header.{payload}.signature");
        assert!(identity::request_for_bearer(&fixture.auth, token.clone())
            .await
            .unwrap()
            .is_none());
        assert!(identity::resolve_subject(&fixture.auth, &metadata, &token)
            .await
            .is_err());
        assert_eq!(fixture.control.requests.load(Ordering::SeqCst), before);
    }
    let ordinary = identity::request_for_bearer(&fixture.auth, "access".into())
        .await
        .unwrap()
        .unwrap();
    assert!(ordinary.authorization.allows(Capability::UseOwnSandboxes));
    assert_eq!(ordinary.subject, layer2_core::SubjectId::new("owner"));
    assert!(fixture.control.requests.load(Ordering::SeqCst) > before);
}

#[tokio::test]
async fn unusable_durable_access_never_reuses_stored_authority_or_calls_provider() {
    let fixture = Fixture::new().await;
    let original = fixture.seed().await;
    for case in 0..3 {
        let mut state = original.clone();
        match case {
            0 => state.tokens = None,
            1 => state.tokens.as_mut().unwrap().expires_at = SystemTime::UNIX_EPOCH,
            _ => state.tokens.as_mut().unwrap().access_token = SecretValue::new("  ".into()),
        }
        fixture.store.put_subject(state.clone()).await.unwrap();
        let request = fixture.request(state.clone()).await.unwrap();
        assert_eq!(request.subject, state.subject);
        assert_eq!(request.authorization, Authorization::none());
        assert_eq!(request.profile, state.profile);
        assert!(request.delegated_credential.is_none());
        fixture.assert_unchanged(&state).await;
    }
    assert_eq!(fixture.control.requests.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn malformed_or_missing_active_introspection_never_uses_partial_or_userinfo_grants() {
    let fixture = Fixture::new().await;
    let state = fixture.seed().await;
    for body in [
        json!({"permissions": ["cma:*"]}),
        json!({"active": "true", "permissions": ["cma:*"]}),
        json!({"active": true}),
        json!({"active": true, "permissions": "cma:*"}),
        json!({"active": true, "permissions": ["cma:*", 17]}),
        json!({"active": true, "sub": null, "permissions": ["cma:*"]}),
    ] {
        *fixture.control.introspection.write().await = Reply::json(body);
        assert_unavailable(fixture.request(state.clone()).await);
        fixture.assert_unchanged(&state).await;
    }
    fixture.control.introspection.write().await.body = "{".into();
    assert_unavailable(fixture.request(state.clone()).await);
    fixture.assert_unchanged(&state).await;
}

#[tokio::test]
async fn userinfo_and_introspection_subjects_must_match_the_exact_cookie_owner() {
    let fixture = Fixture::new().await;
    let state = fixture.seed().await;
    for subject in ["other", " owner "] {
        *fixture.control.userinfo.write().await = Reply::json(json!({"sub": subject}));
        *fixture.control.introspection.write().await = Reply::json(json!({
            "active": true, "sub": subject, "permissions": ["cma:*"],
        }));
        assert_unavailable(fixture.request(state.clone()).await);
    }
    *fixture.control.userinfo.write().await = Reply::json(json!({"sub": "owner"}));
    *fixture.control.introspection.write().await = Reply::json(json!({
        "active": true, "sub": "other", "permissions": ["cma:*"],
    }));
    assert_unavailable(fixture.request(state.clone()).await);
    fixture.assert_unchanged(&state).await;

    let calls = fixture.control.requests.load(Ordering::SeqCst);
    assert_unavailable(request_for_cookie(&fixture.auth, &SubjectId::new("other"), state).await);
    assert_eq!(fixture.control.requests.load(Ordering::SeqCst), calls);
}

#[tokio::test]
async fn provider_outages_are_sanitized_and_leave_durable_authority_unchanged() {
    let fixture = Fixture::new().await;
    let state = fixture.seed().await;
    for endpoint in [
        &fixture.control.discovery,
        &fixture.control.userinfo,
        &fixture.control.introspection,
    ] {
        endpoint.write().await.status = StatusCode::SERVICE_UNAVAILABLE;
        assert_unavailable(fixture.request(state.clone()).await);
        endpoint.write().await.status = StatusCode::OK;
        fixture.assert_unchanged(&state).await;
    }
    let mut metadata = fixture.auth.metadata().await.unwrap();
    metadata.userinfo_endpoint = Some("http://127.0.0.1:0/userinfo".into());
    *fixture.auth.fixture_metadata().write().await = Some(metadata);
    assert_unavailable(fixture.request(state.clone()).await);
    fixture.assert_unchanged(&state).await;
}

#[tokio::test]
async fn unadvertised_introspection_uses_only_complete_userinfo_permission_evidence() {
    let fixture = Fixture::new().await;
    let state = fixture.seed().await;
    let mut metadata = fixture.auth.metadata().await.unwrap();
    metadata.introspection_endpoint = None;
    *fixture.auth.fixture_metadata().write().await = Some(metadata);
    for body in [
        json!({"sub": "owner"}),
        json!({"sub": "owner", "permissions": ["cma:*", false]}),
    ] {
        *fixture.control.userinfo.write().await = Reply::json(body);
        assert_unavailable(fixture.request(state.clone()).await);
    }
    *fixture.control.userinfo.write().await =
        Reply::json(json!({"sub": "owner", "permissions": ["cma:profile:manage"]}));
    let request = fixture.request(state.clone()).await.unwrap();
    assert_eq!(
        request.authorization,
        Authorization::new([Capability::ManageOwnProfiles])
    );
    assert!(request.delegated_credential.is_some());
    fixture.assert_unchanged(&state).await;
}

#[tokio::test]
async fn refreshed_generation_rejects_inflight_evidence_even_when_access_text_is_reused() {
    let fixture = Fixture::new().await;
    let state = fixture.seed().await;
    let (gate, pending) = fixture.blocked_request(state.clone()).await;
    let mut refreshed = state.clone();
    refreshed.revision += 1;
    refreshed.token_generation += 1;
    assert!(fixture
        .store
        .replace_subject(&state.incarnation, state.revision, refreshed.clone())
        .await
        .unwrap());
    gate.release.notify_one();
    assert_unavailable(pending.await.unwrap());
    fixture.assert_unchanged(&refreshed).await;
}

#[tokio::test]
async fn new_login_rejects_inflight_evidence_without_overwriting_the_login() {
    let fixture = Fixture::new().await;
    let state = fixture.seed().await;
    let (gate, pending) = fixture.blocked_request(state.clone()).await;
    fixture
        .auth
        .store_sign_in(
            state.subject.clone(),
            state.tokens.clone().unwrap(),
            Authorization::none(),
            Profile::default(),
        )
        .await
        .unwrap();
    let winner = fixture
        .store
        .subject(&state.subject)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(winner.incarnation, state.incarnation);
    gate.release.notify_one();
    assert_unavailable(pending.await.unwrap());
    fixture.assert_unchanged(&winner).await;
}

#[tokio::test]
async fn recreated_subject_rejects_inflight_evidence_with_equal_generation_and_tokens() {
    let fixture = Fixture::new().await;
    let state = fixture.seed().await;
    let (gate, pending) = fixture.blocked_request(state.clone()).await;
    let recreated = SubjectAuthState::new(
        state.subject.clone(),
        Authorization::none(),
        Profile::default(),
        state.tokens.clone(),
    );
    assert_eq!(recreated.token_generation, state.token_generation);
    fixture.store.put_subject(recreated.clone()).await.unwrap();
    gate.release.notify_one();
    assert_unavailable(pending.await.unwrap());
    fixture.assert_unchanged(&recreated).await;
}

#[tokio::test]
async fn durable_recheck_requires_the_same_still_usable_access_token() {
    let fixture = Fixture::new().await;
    for case in 0..3 {
        let state = fixture.seed().await;
        let (gate, pending) = fixture.blocked_request(state.clone()).await;
        let mut changed = state;
        match case {
            0 => changed.tokens = None,
            1 => changed.tokens.as_mut().unwrap().expires_at = SystemTime::UNIX_EPOCH,
            _ => changed.tokens.as_mut().unwrap().access_token = SecretValue::new("other".into()),
        }
        fixture.store.put_subject(changed.clone()).await.unwrap();
        gate.release.notify_one();
        assert_unavailable(pending.await.unwrap());
        fixture.assert_unchanged(&changed).await;
    }
}

#[tokio::test]
async fn provider_deadline_covers_discovery_userinfo_and_introspection_together() {
    let fixture = Fixture::new().await;
    let state = fixture.seed().await;
    fixture.control.discovery.write().await.delay = Duration::from_millis(80);
    fixture.control.userinfo.write().await.delay = Duration::from_millis(80);
    // Discovery remains serial, followed by concurrent provider reads. Their total
    // must still be bounded by the same per-caller deadline.
    fixture.control.introspection.write().await.delay = Duration::from_millis(160);
    assert_eq!(
        PROVIDER_VERIFICATION_TIMEOUT,
        crate::auth::identity_retry::DEADLINE
    );
    assert_unavailable(
        request_with_timeout(
            &fixture.auth,
            &state.subject,
            state.clone(),
            Duration::from_millis(200),
        )
        .await,
    );
    fixture.assert_unchanged(&state).await;
}
