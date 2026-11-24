use super::*;
use axum::body::{to_bytes, Body};
use axum::http::Request;
use tower::ServiceExt;

pub(super) async fn browser(fixture: &Fixture) -> (Router, String) {
    let login = fixture
        .auth
        .issue_browser_login(SubjectId::new("owner"))
        .await
        .unwrap();
    let app = crate::auth::router(fixture.auth.clone())
        .route("/api/v1/admin/settings", get(|| async { StatusCode::OK }))
        .route("/api/v1/agents", get(|| async { StatusCode::OK }))
        .route("/api/v1/agents/{id}", get(|| async { StatusCode::OK }))
        .route(
            "/api/v1/agents/provenance",
            get(|| async { StatusCode::OK }),
        )
        .layer(axum::middleware::from_fn_with_state(
            fixture.auth.clone(),
            crate::auth::attach_subject,
        ));
    (app, login)
}

pub(super) async fn call(app: &Router, login: &str, path: &str) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::get(path)
                .header(header::COOKIE, format!("cma_session={login}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 65536).await.unwrap();
    let text = String::from_utf8_lossy(&bytes);
    assert!(!text.contains("must-not-leak"));
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn prior_success_cannot_authorize_session_api_or_background_during_outage() {
    let fixture = Fixture::new().await;
    let state = fixture.seed().await;
    let (app, login) = browser(&fixture).await;
    assert_eq!(call(&app, &login, "/auth/session").await.0, StatusCode::OK);
    fixture.control.userinfo.write().await.status = StatusCode::SERVICE_UNAVAILABLE;
    let before = fixture.control.requests.load(Ordering::SeqCst);
    let (status, body) = call(&app, &login, "/auth/session").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["code"], "identity_unavailable");
    assert_eq!(fixture.control.requests.load(Ordering::SeqCst) - before, 6);
    for path in ["/api/v1/admin/settings", "/api/v1/agents"] {
        let (status, body) = call(&app, &login, path).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["code"], "identity_unavailable");
        assert_eq!(body["retryable"], true);
    }
    assert_unavailable(fixture.request(state.clone()).await);
    fixture.control.userinfo.write().await.status = StatusCode::OK;
    *fixture.control.introspection.write().await =
        Reply::json(json!({"active": true, "sub": "owner", "permissions": []}));
    let (status, body) = call(&app, &login, "/auth/session").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["capabilities"], json!([]));
}

#[tokio::test]
async fn rejection_in_either_endpoint_invalidates_warm_record_even_if_the_other_is_unavailable() {
    for stage in ["userinfo", "introspection"] {
        for rejection in [401, 403, 200] {
            let fixture = Fixture::new().await;
            fixture.seed().await;
            let (app, login) = browser(&fixture).await;
            assert_eq!(call(&app, &login, "/auth/session").await.0, StatusCode::OK);
            let (reject, other) = if stage == "userinfo" {
                (&fixture.control.userinfo, &fixture.control.introspection)
            } else {
                (&fixture.control.introspection, &fixture.control.userinfo)
            };
            // active:false is an introspection disposition.
            if rejection == 200 && stage == "userinfo" {
                continue;
            }
            other.write().await.status = StatusCode::SERVICE_UNAVAILABLE;
            reject.write().await.status = StatusCode::from_u16(rejection).unwrap();
            if rejection == 200 {
                reject.write().await.body = json!({"active": false}).to_string();
            }
            let before = fixture.control.requests.load(Ordering::SeqCst);
            assert_eq!(
                call(&app, &login, "/auth/session").await.0,
                StatusCode::UNAUTHORIZED
            );
            assert!(fixture.control.requests.load(Ordering::SeqCst) - before <= 2);
            reject.write().await.status = StatusCode::SERVICE_UNAVAILABLE;
            let (status, body) = call(&app, &login, "/auth/session").await;
            assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
            assert_eq!(body["code"], "identity_unavailable");
        }
    }
}

#[tokio::test]
async fn malformed_evidence_and_nonretryable_http_errors_never_retry_or_fallback() {
    for status in [400, 404, 500, 504, 200] {
        let fixture = Fixture::new().await;
        fixture.seed().await;
        let (app, login) = browser(&fixture).await;
        assert_eq!(call(&app, &login, "/auth/session").await.0, StatusCode::OK);
        fixture.control.userinfo.write().await.status = StatusCode::from_u16(status).unwrap();
        if status == 200 {
            fixture.control.userinfo.write().await.body = "{".into();
        }
        let before = fixture.control.requests.load(Ordering::SeqCst);
        assert_eq!(
            call(&app, &login, "/auth/session").await.0,
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert!(fixture.control.requests.load(Ordering::SeqCst) - before <= 2);
        fixture.control.userinfo.write().await.status = StatusCode::SERVICE_UNAVAILABLE;
        assert_eq!(
            call(&app, &login, "/auth/session").await.0,
            StatusCode::SERVICE_UNAVAILABLE
        );
    }
}

#[tokio::test]
async fn logout_clears_records_and_does_not_restore_them_from_pending_success() {
    let fixture = Fixture::new().await;
    let state = fixture.seed().await;
    let (app, login) = browser(&fixture).await;
    assert_eq!(call(&app, &login, "/auth/session").await.0, StatusCode::OK);
    let (gate, pending) = fixture.blocked_request(state.clone()).await;
    fixture.auth.revoke_browser_login(&login).await.unwrap();
    gate.release.notify_one();
    assert_unavailable(pending.await.unwrap());
    fixture.control.introspection.write().await.gate = None;
    // A new browser cookie with the same retained access token must verify anew.
    let new_login = fixture
        .auth
        .issue_browser_login(state.subject)
        .await
        .unwrap();
    fixture.control.userinfo.write().await.status = StatusCode::SERVICE_UNAVAILABLE;
    assert_eq!(
        call(&app, &new_login, "/auth/session").await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
}

#[tokio::test]
async fn overlapping_session_and_sensitive_calls_share_retries_and_fail_closed() {
    let fixture = Fixture::new().await;
    let state = fixture.seed().await;
    let (app, login) = browser(&fixture).await;
    assert_eq!(call(&app, &login, "/auth/session").await.0, StatusCode::OK);
    let gate = Arc::new(Gate::default());
    fixture.control.userinfo.write().await.gate = Some(gate.clone());
    fixture.control.userinfo.write().await.status = StatusCode::BAD_GATEWAY;
    let before = fixture.control.requests.load(Ordering::SeqCst);
    let requests: Vec<_> = (0..12)
        .map(|index| {
            let (app, login) = (app.clone(), login.clone());
            tokio::spawn(async move {
                (
                    index,
                    call(
                        &app,
                        &login,
                        if index % 2 == 0 {
                            "/auth/session"
                        } else {
                            "/api/v1/admin/settings"
                        },
                    )
                    .await
                    .0,
                )
            })
        })
        .collect();
    concurrency_tests::participants(&fixture.auth, &state, 12).await;
    fixture.control.userinfo.write().await.gate = None;
    gate.release.notify_one();
    for pending in requests {
        let (_, status) = pending.await.unwrap();
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    }
    assert_eq!(fixture.control.requests.load(Ordering::SeqCst) - before, 6);
}

#[tokio::test]
async fn prior_success_never_authorizes_an_outage_before_or_after_sixty_seconds() {
    let fixture = Fixture::new().await;
    fixture.seed().await;
    let (app, login) = browser(&fixture).await;
    assert_eq!(call(&app, &login, "/auth/session").await.0, StatusCode::OK);
    fixture.control.userinfo.write().await.status = StatusCode::SERVICE_UNAVAILABLE;
    for age in [30, 29, 2] {
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(age)).await;
        tokio::time::resume();
        let status = call(&app, &login, "/auth/session").await.0;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    }
    fixture.control.userinfo.write().await.status = StatusCode::OK;
    assert_eq!(call(&app, &login, "/auth/session").await.0, StatusCode::OK);
    fixture.control.userinfo.write().await.status = StatusCode::SERVICE_UNAVAILABLE;
    assert_eq!(
        call(&app, &login, "/auth/session").await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
}

#[tokio::test]
async fn retry_recovers_without_fallback_and_stops_immediately_after_success() {
    let fixture = Fixture::new().await;
    fixture.seed().await;
    let (app, login) = browser(&fixture).await;
    fixture.auth.metadata().await.unwrap();
    let gate = Arc::new(Gate::default());
    fixture.control.userinfo.write().await.gate = Some(gate.clone());
    fixture.control.userinfo.write().await.status = StatusCode::BAD_GATEWAY;
    let before = fixture.control.requests.load(Ordering::SeqCst);
    let pending = tokio::spawn(async move { call(&app, &login, "/auth/session").await });
    gate.reached.notified().await;
    fixture.control.userinfo.write().await.status = StatusCode::OK;
    fixture.control.userinfo.write().await.gate = None;
    gate.release.notify_one();
    assert_eq!(pending.await.unwrap().0, StatusCode::OK);
    assert_eq!(fixture.control.requests.load(Ordering::SeqCst) - before, 4);
}

#[tokio::test]
async fn timeout_and_connection_failure_are_bounded_without_retained_success() {
    for connection in [false, true] {
        let fixture = Fixture::new().await;
        fixture.seed().await;
        let (app, login) = browser(&fixture).await;
        assert_eq!(call(&app, &login, "/auth/session").await.0, StatusCode::OK);
        if connection {
            fixture
                .auth
                .fixture_metadata()
                .write()
                .await
                .as_mut()
                .unwrap()
                .userinfo_endpoint = Some("http://127.0.0.1:0/userinfo".into());
        } else {
            fixture.control.userinfo.write().await.delay = Duration::from_secs(10);
        }
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(3), call(&app, &login, "/auth/session"))
                .await
                .unwrap()
                .0,
            StatusCode::SERVICE_UNAVAILABLE
        );
        fixture.auth.clear_fixture_identity_store();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(3), call(&app, &login, "/auth/session"))
                .await
                .unwrap()
                .0,
            StatusCode::SERVICE_UNAVAILABLE
        );
    }
}

#[tokio::test]
async fn local_capacity_is_fail_closed_without_provider_io_or_fallback() {
    let fixture = Fixture::new().await;
    fixture.seed().await;
    let (app, login) = browser(&fixture).await;
    assert_eq!(call(&app, &login, "/auth/session").await.0, StatusCode::OK);
    let _slots = fixture
        .auth
        .fixture_verification_slots()
        .acquire_many(64)
        .await
        .unwrap();
    let before = fixture.control.requests.load(Ordering::SeqCst);
    assert_eq!(
        call(&app, &login, "/auth/session").await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(fixture.control.requests.load(Ordering::SeqCst), before);
}

#[tokio::test]
async fn expired_provider_evidence_cannot_be_replaced_by_prior_success() {
    let fixture = Fixture::new().await;
    fixture.seed().await;
    let (app, login) = browser(&fixture).await;
    let expiry = (SystemTime::now() + Duration::from_secs(2))
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    *fixture.control.introspection.write().await =
        Reply::json(json!({"active":true, "sub":"owner", "permissions":[], "exp":expiry}));
    assert_eq!(call(&app, &login, "/auth/session").await.0, StatusCode::OK);
    fixture.control.userinfo.write().await.status = StatusCode::SERVICE_UNAVAILABLE;
    fixture.control.introspection.write().await.status = StatusCode::SERVICE_UNAVAILABLE;
    let remaining = (SystemTime::UNIX_EPOCH + Duration::from_secs(expiry))
        .duration_since(SystemTime::now())
        .unwrap_or_default();
    tokio::time::sleep(remaining + Duration::from_millis(10)).await;
    assert_eq!(
        call(&app, &login, "/auth/session").await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
}

#[tokio::test]
async fn partial_success_cannot_hide_invalid_evidence_or_changed_subject() {
    for body in [
        json!({"active":true}),
        json!({"active":"true","permissions":[]}),
        json!({"active":true,"sub":"another-owner","permissions":[]}),
    ] {
        let fixture = Fixture::new().await;
        fixture.seed().await;
        let (app, login) = browser(&fixture).await;
        assert_eq!(call(&app, &login, "/auth/session").await.0, StatusCode::OK);
        fixture.control.userinfo.write().await.status = StatusCode::SERVICE_UNAVAILABLE;
        let mismatch = body["sub"] == "another-owner";
        *fixture.control.introspection.write().await = Reply::json(body);
        assert_eq!(
            call(&app, &login, "/auth/session").await.0,
            if mismatch {
                StatusCode::UNAUTHORIZED
            } else {
                StatusCode::SERVICE_UNAVAILABLE
            }
        );
        fixture.control.introspection.write().await.status = StatusCode::SERVICE_UNAVAILABLE;
        assert_eq!(
            call(&app, &login, "/auth/session").await.0,
            StatusCode::SERVICE_UNAVAILABLE
        );
    }
}

#[tokio::test]
async fn rejected_refresh_grant_erases_custody_and_signs_out_immediately() {
    let fixture = Fixture::new().await;
    let mut state = fixture.seed().await;
    let (app, login) = browser(&fixture).await;
    assert_eq!(call(&app, &login, "/auth/session").await.0, StatusCode::OK);
    assert_eq!(fixture.auth.fixture_active_receipts(), 0);
    state.tokens.as_mut().unwrap().expires_at = SystemTime::now() + Duration::from_secs(30);
    fixture.store.put_subject(state).await.unwrap();
    fixture.control.exchange.write().await.status = StatusCode::BAD_REQUEST;
    fixture.control.exchange.write().await.body = json!({"error":"invalid_grant"}).to_string();
    assert_eq!(
        call(&app, &login, "/auth/session").await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(fixture.auth.fixture_active_receipts(), 0);
    assert!(fixture
        .store
        .subject(&SubjectId::new("owner"))
        .await
        .unwrap()
        .unwrap()
        .tokens
        .is_none());
}
