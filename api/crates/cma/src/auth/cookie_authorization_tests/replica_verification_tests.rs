//! Independent replicas always obtain new provider evidence after a flight ends.
use super::{fallback_tests::browser, *};
use axum::{body::Body, http::Request};
use tower::ServiceExt;

async fn read(app: &Router, login: &str, bearer: bool) -> StatusCode {
    let request = Request::get("/auth/session");
    let request = if bearer {
        request.header(header::AUTHORIZATION, "Bearer must-not-leak-access-token")
    } else {
        request.header(header::COOKIE, format!("cma_session={login}"))
    };
    app.clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn two_replicas_observe_revocation_and_outage_without_borrowing_previous_success() {
    for bearer in [false, true] {
        let fixture = Fixture::new().await;
        fixture.seed().await;
        let other = AuthState::with_store(
            fixture.auth.fixture_mode().clone(),
            None,
            fixture.store.clone(),
        );
        let (first, login) = browser(&fixture).await;
        let second = crate::auth::router(other.clone()).layer(
            axum::middleware::from_fn_with_state(other.clone(), crate::auth::attach_subject),
        );
        for app in [&first, &second] {
            assert_eq!(read(app, &login, bearer).await, StatusCode::OK);
        }
        assert_eq!(fixture.auth.fixture_active_receipts(), 0);
        assert_eq!(other.fixture_active_receipts(), 0);
        *fixture.control.introspection.write().await = Reply::json(json!({"active":false}));
        // Only A has observed refusal when B starts its next independent read.
        assert_eq!(read(&first, &login, bearer).await, StatusCode::UNAUTHORIZED);
        let before = fixture.control.requests.load(Ordering::SeqCst);
        assert_eq!(
            read(&second, &login, bearer).await,
            StatusCode::UNAUTHORIZED
        );
        assert!(fixture.control.requests.load(Ordering::SeqCst) > before);
        assert!(fixture
            .store
            .subject(&SubjectId::new("owner"))
            .await
            .unwrap()
            .unwrap()
            .tokens
            .is_some());
        *fixture.control.introspection.write().await = Reply::json(json!({
            "active":true,"sub":"owner","permissions":["cma:sandbox:use"]
        }));
        for app in [&first, &second] {
            assert_eq!(read(app, &login, bearer).await, StatusCode::OK);
        }
        fixture.control.userinfo.write().await.status = StatusCode::SERVICE_UNAVAILABLE;
        for app in [&first, &second] {
            assert_eq!(
                read(app, &login, bearer).await,
                StatusCode::SERVICE_UNAVAILABLE
            );
        }
    }
}

#[tokio::test]
async fn sequential_reads_always_dispatch_even_with_identical_credentials() {
    let fixture = Fixture::new().await;
    fixture.seed().await;
    fixture.auth.metadata().await.unwrap();
    let (app, login) = browser(&fixture).await;
    let before = fixture.control.requests.load(Ordering::SeqCst);
    for _ in 0..40 {
        assert_eq!(read(&app, &login, false).await, StatusCode::OK);
    }
    let calls = fixture.control.requests.load(Ordering::SeqCst) - before;
    assert_eq!(calls, 80);
    assert_eq!(fixture.auth.fixture_active_receipts(), 0);
    println!("identity_live_calls requests=40 provider_calls={calls} discovery=prewarmed");
}
