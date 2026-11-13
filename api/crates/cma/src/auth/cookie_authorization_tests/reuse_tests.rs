//! Loopback contracts exercise reuse through the real HTTP authentication layer.
use super::{
    fallback_tests::{browser, call},
    *,
};
use axum::{
    body::{to_bytes, Body},
    http::{Method, Request},
};
use tower::ServiceExt;

async fn enabled() -> Fixture {
    let mut fixture = Fixture::new().await;
    Arc::get_mut(&mut fixture.auth).unwrap().identity_reuse_age = Duration::from_secs(5);
    fixture.seed().await;
    fixture
}

#[tokio::test]
async fn forty_reads_reuse_one_success_without_extending_the_ttl() {
    for enabled in [false, true] {
        let mut fixture = Fixture::new().await;
        if enabled {
            Arc::get_mut(&mut fixture.auth).unwrap().identity_reuse_age = Duration::from_secs(5);
        }
        fixture.seed().await;
        fixture.auth.metadata().await.unwrap();
        let (app, login) = browser(&fixture).await;
        let before = fixture.control.requests.load(Ordering::SeqCst);
        for _ in 0..40 {
            assert_eq!(call(&app, &login, "/auth/session").await.0, StatusCode::OK);
        }
        let calls = fixture.control.requests.load(Ordering::SeqCst) - before;
        assert_eq!(calls, if enabled { 2 } else { 80 });
        println!("identity_reuse_calls requests=40 enabled={enabled} provider_calls={calls}");
        if enabled {
            *fixture.control.introspection.write().await = Reply::json(json!({"active":false}));
            // An undelivered provider revocation is visible at TTL, not magically
            // at its source. Read hits must never slide that boundary forward.
            tokio::time::pause();
            tokio::time::advance(Duration::from_secs(4)).await;
            assert_eq!(call(&app, &login, "/auth/session").await.0, StatusCode::OK);
            tokio::time::advance(Duration::from_secs(1)).await;
            tokio::time::resume();
            assert_eq!(
                call(&app, &login, "/auth/session").await.0,
                StatusCode::UNAUTHORIZED
            );
        }
    }
}

#[tokio::test]
async fn observed_revocation_invalidates_success_before_the_ttl() {
    let fixture = enabled().await;
    let state = fixture
        .store
        .subject(&SubjectId::new("owner"))
        .await
        .unwrap()
        .unwrap();
    let (app, login) = browser(&fixture).await;
    assert_eq!(call(&app, &login, "/auth/session").await.0, StatusCode::OK);
    *fixture.control.introspection.write().await = Reply::json(json!({"active":false}));
    // A live command/background verification observes the provider signal.
    assert_unavailable(fixture.request(state).await);
    assert_eq!(
        call(&app, &login, "/auth/session").await.0,
        StatusCode::UNAUTHORIZED
    );
    let key = fixture.auth.identity_store.key(
        CredentialPurpose::BrowserCookie,
        "must-not-leak-access-token",
    );
    assert!(fixture
        .auth
        .identity_store
        .get(&key, &fixture.auth.identity_store.epoch())
        .is_none());
}

#[tokio::test]
async fn logout_and_token_replacement_do_not_borrow_a_cached_identity() {
    for logout in [false, true] {
        let fixture = enabled().await;
        let (app, login) = browser(&fixture).await;
        assert_eq!(call(&app, &login, "/auth/session").await.0, StatusCode::OK);
        if logout {
            // Shared durable custody changes as it would on another replica.
            fixture
                .store
                .delete_browser_login(&SecretDigest::identify("browser-login", &login))
                .await
                .unwrap();
        } else {
            let mut state = fixture
                .store
                .subject(&SubjectId::new("owner"))
                .await
                .unwrap()
                .unwrap();
            state.revision += 1;
            state.token_generation += 1;
            state.tokens.as_mut().unwrap().access_token =
                SecretValue::new("replaced-fixture-access".into());
            fixture.store.put_subject(state).await.unwrap();
            *fixture.control.introspection.write().await = Reply::json(json!({"active":false}));
        }
        assert_eq!(
            call(&app, &login, "/auth/session").await.0,
            StatusCode::UNAUTHORIZED
        );
    }
}

#[tokio::test]
async fn writes_and_privileged_reads_always_observe_live_revocation() {
    for (method, path) in [
        (Method::POST, "/api/v1/agents"),
        (Method::GET, "/api/v1/admin/settings"),
        (Method::GET, "/api/v1/agents/provenance"),
    ] {
        let fixture = enabled().await;
        let (app, login) = browser(&fixture).await;
        assert_eq!(call(&app, &login, "/auth/session").await.0, StatusCode::OK);
        let before = fixture.control.requests.load(Ordering::SeqCst);
        *fixture.control.introspection.write().await = Reply::json(json!({"active":false}));
        let response = app
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header(header::COOKIE, format!("cma_session={login}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        // Even if the route has no matching method, auth must have gone online.
        assert!(fixture.control.requests.load(Ordering::SeqCst) > before);
        let body = to_bytes(response.into_body(), 65536).await.unwrap();
        assert!(!String::from_utf8_lossy(&body).contains("must-not-leak"));
        let key = fixture.auth.identity_store.key(
            CredentialPurpose::BrowserCookie,
            "must-not-leak-access-token",
        );
        assert!(fixture
            .auth
            .identity_store
            .get(&key, &fixture.auth.identity_store.epoch())
            .is_none());
    }
}

#[tokio::test]
async fn detail_reuse_follows_the_matched_route_template() {
    let fixture = enabled().await;
    let (app, login) = browser(&fixture).await;
    assert_eq!(
        call(&app, &login, "/api/v1/agents/example").await.0,
        StatusCode::OK
    );
    let before = fixture.control.requests.load(Ordering::SeqCst);
    assert_eq!(
        call(&app, &login, "/api/v1/agents/example").await.0,
        StatusCode::OK
    );
    assert_eq!(fixture.control.requests.load(Ordering::SeqCst), before);
    *fixture.control.introspection.write().await = Reply::json(json!({"active":false}));
    call(&app, &login, "/api/v1/agents/provenance").await;
    assert!(fixture.control.requests.load(Ordering::SeqCst) > before);
    assert!(fixture
        .auth
        .identity_store
        .get(
            &fixture.auth.identity_store.key(
                CredentialPurpose::BrowserCookie,
                "must-not-leak-access-token"
            ),
            &fixture.auth.identity_store.epoch(),
        )
        .is_none());
}

#[tokio::test]
async fn expiry_or_invalidation_during_authorization_io_rejects_reuse() {
    for invalidate in [false, true] {
        let fixture = enabled().await;
        let state = fixture
            .store
            .subject(&SubjectId::new("owner"))
            .await
            .unwrap()
            .unwrap();
        fixture.request(state.clone()).await.unwrap();
        let result = crate::auth::identity_reuse::scope(
            &fixture.auth,
            &Method::GET,
            "/api/v1/agents",
            async {
                let verified = fixture.request(state).await?;
                if invalidate {
                    fixture
                        .auth
                        .identity_store
                        .invalidate_token("must-not-leak-access-token");
                } else {
                    tokio::time::pause();
                    tokio::time::advance(Duration::from_secs(5)).await;
                    tokio::time::resume();
                }
                Ok(verified)
            },
        )
        .await;
        assert_unavailable(result);
    }
}

#[tokio::test]
async fn pending_success_cannot_repopulate_after_observed_invalidation() {
    let fixture = enabled().await;
    let state = fixture
        .store
        .subject(&SubjectId::new("owner"))
        .await
        .unwrap()
        .unwrap();
    let (gate, pending) = fixture.blocked_request(state).await;
    fixture
        .auth
        .identity_store
        .invalidate_token("must-not-leak-access-token");
    gate.release.notify_one();
    assert_unavailable(pending.await.unwrap());
    let key = fixture.auth.identity_store.key(
        CredentialPurpose::BrowserCookie,
        "must-not-leak-access-token",
    );
    assert!(fixture
        .auth
        .identity_store
        .get(&key, &fixture.auth.identity_store.epoch())
        .is_none());
}

#[tokio::test]
async fn bearer_reads_share_success_but_never_cross_credential_purpose() {
    let fixture = enabled().await;
    let mut body: Value =
        serde_json::from_str(&fixture.control.introspection.read().await.body).unwrap();
    body["exp"] = json!((SystemTime::now() + Duration::from_secs(3600))
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs());
    *fixture.control.introspection.write().await = Reply::json(body);
    fixture.auth.metadata().await.unwrap();
    let before = fixture.control.requests.load(Ordering::SeqCst);
    for _ in 0..3 {
        crate::auth::identity_reuse::scope(&fixture.auth, &Method::GET, "/api/v1/agents", async {
            identity::request_for_bearer(&fixture.auth, "must-not-leak-access-token".into()).await
        })
        .await
        .unwrap()
        .unwrap();
    }
    assert_eq!(fixture.control.requests.load(Ordering::SeqCst) - before, 2);
    let (app, login) = browser(&fixture).await;
    assert_eq!(call(&app, &login, "/auth/session").await.0, StatusCode::OK);
    assert_eq!(fixture.control.requests.load(Ordering::SeqCst) - before, 4);
}
