//! Connection checks use the same bounded fresh identity verifier as API reads.
use super::*;
use axum::{body::Body, http::Request};
use tower::ServiceExt;

fn app(fixture: &Fixture, calls: Arc<AtomicUsize>) -> Router {
    Router::new()
        .route(
            "/auth/connections/check",
            post(move |_: layer1_public::AuthenticatedCaller| {
                let calls = calls.clone();
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    StatusCode::OK
                }
            }),
        )
        .layer(axum::middleware::from_fn_with_state(
            fixture.auth.clone(),
            crate::auth::attach_subject,
        ))
}

fn request(login: &str) -> Request<Body> {
    Request::post("/auth/connections/check")
        .header(header::COOKIE, format!("cma_session={login}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"target":{"type":"agent","id":"agent"}}"#))
        .unwrap()
}

#[tokio::test]
async fn connection_check_retries_throttled_identity_before_entering_the_handler() {
    let fixture = Fixture::new().await;
    fixture.seed().await;
    fixture.auth.metadata().await.unwrap();
    let login = fixture
        .auth
        .issue_browser_login(SubjectId::new("owner"))
        .await
        .unwrap();
    let gate = Arc::new(Gate::default());
    {
        let mut reply = fixture.control.introspection.write().await;
        reply.status = StatusCode::TOO_MANY_REQUESTS;
        reply
            .headers
            .insert(header::RETRY_AFTER, "1".parse().unwrap());
        reply.gate = Some(gate.clone());
    }
    let calls = Arc::new(AtomicUsize::new(0));
    let app = app(&fixture, calls.clone());
    let before = fixture.control.requests.load(Ordering::SeqCst);
    let started = std::time::Instant::now();
    let pending = tokio::spawn(async move { app.oneshot(request(&login)).await.unwrap() });
    gate.reached.notified().await;
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    {
        let mut reply = fixture.control.introspection.write().await;
        reply.status = StatusCode::OK;
        reply.gate = None;
    }
    gate.release.notify_one();
    assert_eq!(pending.await.unwrap().status(), StatusCode::OK);
    assert!(started.elapsed() >= Duration::from_secs(1));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.control.requests.load(Ordering::SeqCst) - before, 4);
}

#[tokio::test]
async fn connection_check_preserves_long_identity_cooldowns_without_handler_effects() {
    let fixture = Fixture::new().await;
    fixture.seed().await;
    fixture.auth.metadata().await.unwrap();
    let login = fixture
        .auth
        .issue_browser_login(SubjectId::new("owner"))
        .await
        .unwrap();
    {
        let mut reply = fixture.control.introspection.write().await;
        reply.status = StatusCode::TOO_MANY_REQUESTS;
        reply
            .headers
            .insert(header::RETRY_AFTER, "60".parse().unwrap());
    }
    let calls = Arc::new(AtomicUsize::new(0));
    let before = fixture.control.requests.load(Ordering::SeqCst);
    let response = app(&fixture, calls.clone())
        .oneshot(request(&login))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(response.headers()[header::RETRY_AFTER], "60");
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.control.requests.load(Ordering::SeqCst) - before, 2);
}
