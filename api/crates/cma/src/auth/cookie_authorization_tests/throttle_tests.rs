use super::{
    fallback_tests::{browser, call},
    *,
};
use axum::{
    body::{to_bytes, Body},
    http::Request,
};
use tower::ServiceExt;

async fn throttle(reply: &RwLock<Reply>, after: &str) {
    let mut reply = reply.write().await;
    reply.status = StatusCode::TOO_MANY_REQUESTS;
    reply
        .headers
        .insert(header::RETRY_AFTER, after.parse().unwrap());
}

#[tokio::test]
async fn retry_after_recovers_once_the_provider_accepts_the_request() {
    let fixture = Fixture::new().await;
    fixture.seed().await;
    let (app, login) = browser(&fixture).await;
    fixture.auth.metadata().await.unwrap();
    throttle(&fixture.control.introspection, "1").await;
    let gate = Arc::new(Gate::default());
    fixture.control.introspection.write().await.gate = Some(gate.clone());
    let before = fixture.control.requests.load(Ordering::SeqCst);
    let started = std::time::Instant::now();
    let pending = tokio::spawn(async move { call(&app, &login, "/auth/session").await });
    gate.reached.notified().await;
    {
        let mut reply = fixture.control.introspection.write().await;
        reply.status = StatusCode::OK;
        reply.gate = None;
    }
    gate.release.notify_one();
    assert_eq!(pending.await.unwrap().0, StatusCode::OK);
    assert!(
        started.elapsed() >= Duration::from_secs(1),
        "must not shorten Retry-After"
    );
    assert!(started.elapsed() < Duration::from_secs(3));
    assert_eq!(fixture.control.requests.load(Ordering::SeqCst) - before, 4);
}

#[tokio::test]
async fn long_backoff_returns_typed_retryable_503_on_api_and_session_without_retrying() {
    for path in ["/auth/session", "/api/v1/agents"] {
        let fixture = Fixture::new().await;
        fixture.seed().await;
        let (app, login) = browser(&fixture).await;
        assert_eq!(call(&app, &login, "/auth/session").await.0, StatusCode::OK);
        throttle(&fixture.control.introspection, "60").await;
        let before = fixture.control.requests.load(Ordering::SeqCst);
        let response = app
            .oneshot(
                Request::get(path)
                    .header(header::COOKIE, format!("cma_session={login}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(response.headers()[header::RETRY_AFTER], "60");
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let body: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
        assert_eq!(body["code"], "identity_unavailable");
        assert_eq!(body["retryable"], true);
        assert!(fixture.control.requests.load(Ordering::SeqCst) - before <= 2);
    }
}

#[tokio::test]
async fn zero_delay_still_has_a_finite_attempt_budget_and_no_fallback() {
    let fixture = Fixture::new().await;
    fixture.seed().await;
    let (app, login) = browser(&fixture).await;
    assert_eq!(call(&app, &login, "/auth/session").await.0, StatusCode::OK);
    throttle(&fixture.control.userinfo, "0").await;
    let before = fixture.control.requests.load(Ordering::SeqCst);
    let (status, body) = call(&app, &login, "/auth/session").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["code"], "identity_unavailable");
    assert_eq!(body["retryable"], true);
    assert_eq!(fixture.control.requests.load(Ordering::SeqCst) - before, 6);
}

#[tokio::test]
async fn backoff_without_room_for_a_complete_attempt_preserves_typed_exhaustion() {
    let fixture = Fixture::new().await;
    fixture.seed().await;
    let (app, login) = browser(&fixture).await;
    fixture.auth.metadata().await.unwrap();
    throttle(&fixture.control.introspection, "2").await;
    let before = fixture.control.requests.load(Ordering::SeqCst);
    let (status, body) = call(&app, &login, "/api/v1/agents").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["code"], "identity_unavailable");
    assert_eq!(body["retryable"], true);
    assert_eq!(fixture.control.requests.load(Ordering::SeqCst) - before, 2);
}

#[tokio::test]
async fn explicit_rejection_wins_over_throttling_in_either_completion_order() {
    for userinfo in [false, true] {
        for reject_first in [false, true] {
            let fixture = Fixture::new().await;
            fixture.seed().await;
            let (app, login) = browser(&fixture).await;
            fixture.auth.metadata().await.unwrap();
            let (reject, limited) = if userinfo {
                (&fixture.control.userinfo, &fixture.control.introspection)
            } else {
                (&fixture.control.introspection, &fixture.control.userinfo)
            };
            reject.write().await.status = StatusCode::UNAUTHORIZED;
            throttle(limited, "0").await;
            if reject_first {
                limited.write().await.delay = Duration::from_millis(10);
            } else {
                reject.write().await.delay = Duration::from_millis(10);
            }
            let before = fixture.control.requests.load(Ordering::SeqCst);
            assert_eq!(
                call(&app, &login, "/auth/session").await.0,
                StatusCode::UNAUTHORIZED
            );
            assert!(fixture.control.requests.load(Ordering::SeqCst) - before <= 2);
        }
    }
}

#[tokio::test]
async fn simultaneous_throttles_respect_the_longer_delay() {
    let fixture = Fixture::new().await;
    let state = fixture.seed().await;
    fixture.auth.metadata().await.unwrap();
    throttle(&fixture.control.userinfo, "0").await;
    throttle(&fixture.control.introspection, "60").await;
    let before = fixture.control.requests.load(Ordering::SeqCst);
    assert_unavailable(fixture.request(state).await);
    assert_eq!(fixture.control.requests.load(Ordering::SeqCst) - before, 2);
}

#[tokio::test]
async fn cancelling_a_throttled_flight_releases_capacity_without_detached_retries() {
    let fixture = Fixture::new().await;
    let state = fixture.seed().await;
    fixture.auth.metadata().await.unwrap();
    throttle(&fixture.control.userinfo, "1").await;
    let gate = Arc::new(Gate::default());
    fixture.control.userinfo.write().await.gate = Some(gate.clone());
    let auth = fixture.auth.clone();
    let pending =
        tokio::spawn(
            async move { request_for_cookie(&auth, &SubjectId::new("owner"), state).await },
        );
    gate.reached.notified().await;
    gate.release.notify_one();
    tokio::time::sleep(Duration::from_millis(30)).await;
    pending.abort();
    assert!(matches!(pending.await, Err(error) if error.is_cancelled()));
    let before = fixture.control.requests.load(Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(1050)).await;
    assert_eq!(fixture.control.requests.load(Ordering::SeqCst), before);
    assert_eq!(
        fixture
            .auth
            .fixture_verification_slots()
            .available_permits(),
        64
    );
}

#[tokio::test]
async fn advisory_link_does_not_hide_throttle_type_or_backoff_on_public_routes() {
    for path in ["/auth/session", "/api/v1/agents"] {
        for userinfo in [false, true] {
            let fixture = Fixture::new().await;
            fixture.seed().await;
            let (app, login) = browser(&fixture).await;
            assert_eq!(call(&app, &login, "/auth/session").await.0, StatusCode::OK);
            let endpoint = if userinfo {
                &fixture.control.userinfo
            } else {
                &fixture.control.introspection
            };
            throttle(endpoint, "60").await;
            endpoint.write().await.headers.insert(
                header::LINK,
                "</rate-limit-help>; rel=\"help\"".parse().unwrap(),
            );
            let before = fixture.control.requests.load(Ordering::SeqCst);
            let response = app
                .oneshot(
                    Request::get(path)
                        .header(header::COOKIE, format!("cma_session={login}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
            assert_eq!(response.headers()[header::RETRY_AFTER], "60");
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
            let body: Value =
                serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap())
                    .unwrap();
            assert_eq!(body["code"], "identity_unavailable");
            assert_eq!(body["retryable"], true);
            assert!(fixture.control.requests.load(Ordering::SeqCst) - before <= 2);
        }
    }
}

#[tokio::test]
async fn advisory_link_keeps_refusal_precedence_and_rejects_incomplete_success() {
    for status in [
        StatusCode::UNAUTHORIZED,
        StatusCode::FORBIDDEN,
        StatusCode::OK,
    ] {
        let fixture = Fixture::new().await;
        fixture.seed().await;
        let (app, login) = browser(&fixture).await;
        let mut reply = fixture.control.introspection.write().await;
        reply.status = status;
        reply
            .headers
            .insert(header::LINK, "</next>; rel=\"next\"".parse().unwrap());
        reply
            .headers
            .insert(header::RETRY_AFTER, "60".parse().unwrap());
        drop(reply);
        let response = app
            .oneshot(
                Request::get("/auth/session")
                    .header(header::COOKIE, format!("cma_session={login}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            if status == StatusCode::OK {
                StatusCode::SERVICE_UNAVAILABLE
            } else {
                StatusCode::UNAUTHORIZED
            }
        );
        assert!(!response.headers().contains_key(header::RETRY_AFTER));
    }
}
