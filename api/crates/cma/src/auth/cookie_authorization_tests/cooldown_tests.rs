//! Public middleware regressions for completed-flight negative throttles.
use super::{fallback_tests::browser, *};
use axum::{
    body::{to_bytes, Body},
    http::Request,
};
use tower::ServiceExt;

async fn throttle_both(fixture: &Fixture, after: &'static str) {
    for endpoint in [&fixture.control.userinfo, &fixture.control.introspection] {
        let mut reply = endpoint.write().await;
        reply.status = StatusCode::TOO_MANY_REQUESTS;
        reply
            .headers
            .insert(header::RETRY_AFTER, after.parse().unwrap());
    }
}

async fn read(
    app: &Router,
    login: &str,
    bearer: bool,
    path: &str,
) -> (StatusCode, HeaderMap, Value) {
    let request = Request::get(path);
    let request = if bearer {
        request.header(header::AUTHORIZATION, "Bearer must-not-leak-access-token")
    } else {
        request.header(header::COOKIE, format!("cma_session={login}"))
    };
    let response = app
        .clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = to_bytes(response.into_body(), 65536).await.unwrap();
    assert!(!String::from_utf8_lossy(&bytes).contains("must-not-leak"));
    (
        status,
        headers,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn assert_throttled(response: (StatusCode, HeaderMap, Value)) {
    let (status, headers, body) = response;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    let remaining: u64 = headers[header::RETRY_AFTER]
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    assert!((1..=60).contains(&remaining));
    assert_eq!(headers[header::CACHE_CONTROL], "no-store");
    assert_eq!(body["code"], "identity_unavailable");
    assert_eq!(body["retryable"], true);
}

#[tokio::test]
async fn cookie_and_bearer_bursts_across_completed_flights_share_only_negative_backoff() {
    let fixture = Fixture::new().await;
    fixture.seed().await;
    let (app, login) = browser(&fixture).await;
    assert_eq!(
        read(&app, &login, false, "/auth/session").await.0,
        StatusCode::OK
    );
    throttle_both(&fixture, "60").await;
    let before = fixture.control.requests.load(Ordering::SeqCst);
    let mut tasks = tokio::task::JoinSet::new();
    for index in 0..12 {
        let (app, login) = (app.clone(), login.clone());
        tasks.spawn(
            async move { read(&app, &login, index % 2 == 0, "/api/v1/agents/fixture").await },
        );
    }
    while let Some(result) = tasks.join_next().await {
        assert_throttled(result.unwrap());
    }
    for index in 0..24 {
        assert_throttled(read(&app, &login, index % 2 == 0, "/auth/session").await);
    }
    assert_eq!(fixture.control.requests.load(Ordering::SeqCst) - before, 2);
    assert_eq!(
        fixture
            .auth
            .fixture_verification_slots()
            .available_permits(),
        64
    );
    assert_eq!(fixture.auth.fixture_active_receipts(), 0);
}

#[tokio::test]
async fn revoked_credential_never_reuses_prior_success_during_or_after_cooldown() {
    for bearer in [false, true] {
        let fixture = Fixture::new().await;
        fixture.seed().await;
        let (app, login) = browser(&fixture).await;
        assert_eq!(
            read(&app, &login, bearer, "/auth/session").await.0,
            StatusCode::OK
        );
        throttle_both(&fixture, "60").await;
        assert_throttled(read(&app, &login, bearer, "/auth/session").await);
        fixture.control.userinfo.write().await.status = StatusCode::OK;
        *fixture.control.introspection.write().await = Reply::json(json!({"active": false}));
        // Revocation during a cooldown cannot yield a cached success.
        assert_throttled(read(&app, &login, bearer, "/auth/session").await);
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(61)).await;
        tokio::time::resume();
        assert_eq!(
            read(&app, &login, bearer, "/auth/session").await.0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(fixture.auth.fixture_active_receipts(), 0);
    }
}

#[tokio::test]
async fn rotated_cookie_credential_does_not_inherit_previous_tokens_cooldown() {
    let fixture = Fixture::new().await;
    let mut state = fixture.seed().await;
    let (app, login) = browser(&fixture).await;
    fixture.auth.metadata().await.unwrap();
    throttle_both(&fixture, "60").await;
    assert_throttled(read(&app, &login, false, "/auth/session").await);
    let previous = state.clone();
    state.revision += 1;
    state.token_generation += 1;
    state.tokens.as_mut().unwrap().access_token = SecretValue::new("rotated-access-fixture".into());
    assert!(fixture
        .store
        .replace_subject(&previous.incarnation, previous.revision, state)
        .await
        .unwrap());
    for endpoint in [&fixture.control.userinfo, &fixture.control.introspection] {
        endpoint.write().await.status = StatusCode::OK;
    }
    let before = fixture.control.requests.load(Ordering::SeqCst);
    assert_eq!(
        read(&app, &login, false, "/auth/session").await.0,
        StatusCode::OK
    );
    assert_eq!(fixture.control.requests.load(Ordering::SeqCst) - before, 2);
    assert_throttled(read(&app, &login, true, "/auth/session").await);
}

#[tokio::test]
async fn fresh_refusal_on_other_endpoint_wins_over_retained_throttle() {
    for userinfo in [false, true] {
        let fixture = Fixture::new().await;
        fixture.seed().await;
        let (app, login) = browser(&fixture).await;
        fixture.auth.metadata().await.unwrap();
        let (limited, refused) = if userinfo {
            (&fixture.control.userinfo, &fixture.control.introspection)
        } else {
            (&fixture.control.introspection, &fixture.control.userinfo)
        };
        {
            let mut reply = limited.write().await;
            reply.status = StatusCode::TOO_MANY_REQUESTS;
            reply
                .headers
                .insert(header::RETRY_AFTER, "60".parse().unwrap());
        }
        assert_throttled(read(&app, &login, false, "/auth/session").await);
        refused.write().await.status = StatusCode::UNAUTHORIZED;
        assert_eq!(
            read(&app, &login, false, "/auth/session").await.0,
            StatusCode::UNAUTHORIZED
        );
    }
}

#[tokio::test]
async fn one_owners_throttle_does_not_block_or_authenticate_another_owner() {
    let fixture = Fixture::new().await;
    fixture.seed().await;
    let (app, alice_login) = browser(&fixture).await;
    fixture.auth.metadata().await.unwrap();
    throttle_both(&fixture, "60").await;
    assert_throttled(read(&app, &alice_login, false, "/auth/session").await);
    fixture
        .store
        .put_subject(SubjectAuthState::new(
            SubjectId::new("bob"),
            crate::permissions::capabilities_from_permissions(["cma:*"]),
            Profile::default(),
            Some(DurableSubjectTokens {
                access_token: SecretValue::new("bob-access-fixture".into()),
                refresh_token: None,
                expires_at: SystemTime::now() + Duration::from_secs(3600),
            }),
        ))
        .await
        .unwrap();
    let bob_login = fixture
        .auth
        .issue_browser_login(SubjectId::new("bob"))
        .await
        .unwrap();
    *fixture.control.userinfo.write().await =
        Reply::json(json!({"sub":"bob", "permissions":["cma:*"]}));
    *fixture.control.introspection.write().await =
        Reply::json(json!({"sub":"bob", "active":true, "permissions":["cma:*"]}));
    let (status, _, body) = read(&app, &bob_login, false, "/auth/session").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.to_string().contains("bob"));
    assert!(!body.to_string().contains("Stored Owner"));
    assert_throttled(read(&app, &alice_login, false, "/auth/session").await);
}
