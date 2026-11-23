//! Exercise final attribution through the real request and authentication layers.
use super::*;
use axum::{
    body::{to_bytes, Body},
    http::Request,
};
use tower::ServiceExt;
use tracing::instrument::WithSubscriber;

#[derive(Clone, Default)]
struct Log(Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for Log {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

async fn capture<F: std::future::Future>(future: F) -> (F::Output, Vec<Value>) {
    let log = Log::default();
    let writer = log.clone();
    let subscriber = tracing_subscriber::fmt()
        .json()
        .event_format(crate::diagnostic_format::json())
        .with_max_level(tracing::Level::INFO)
        .with_writer(move || writer.clone())
        .finish();
    let result = future.with_subscriber(subscriber).await;
    let text = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
    let events: Vec<Value> = text
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter(|event| event["fields"]["event"] == "authentication_failure")
        .collect();
    for event in &events {
        assert_eq!(event["fields"]["auth_stage"], "request_resolution");
        assert_eq!(event["fields"]["error_class"], "unavailable");
        let hash = event["fields"]["request_hash"].as_str().unwrap();
        assert_eq!(hash.len(), 64);
        assert!(hash.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert!(event.get("span").is_none());
        assert!(event.get("spans").is_none());
        for private in [
            "must-not-leak",
            "http://",
            "owner",
            "cookie-client",
            "fixture",
        ] {
            assert!(!event.to_string().contains(private), "{event}");
        }
    }
    (result, events)
}

fn app(auth: Arc<AuthState>) -> Router {
    Router::new()
        .route(
            "/api/v1/admin/usage/metrics/view",
            get(|| async { StatusCode::OK }),
        )
        .route("/auth/session", get(|| async { StatusCode::OK }))
        .layer(axum::middleware::from_fn_with_state(
            auth,
            crate::auth::attach_subject,
        ))
        .layer(axum::middleware::from_fn(crate::diagnostic_request::scope))
}

fn request(id: &str, cookie: bool, path: &str) -> Request<Body> {
    let mut request = Request::get(path).header("x-request-id", id);
    if cookie {
        request = request.header(header::COOKIE, "cma_session=must-not-leak-cookie");
    } else {
        request = request.header(header::AUTHORIZATION, "Bearer must-not-leak-bearer");
    }
    request.body(Body::empty()).unwrap()
}

async fn assert_failure(response: Response, retry: Option<&str>) {
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        response
            .headers()
            .get(header::RETRY_AFTER)
            .map(|v| v.to_str().unwrap()),
        retry
    );
    if retry.is_some() {
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    }
    let expected = if retry.is_some() {
        crate::auth::middleware::identity_failure()
    } else {
        crate::auth::middleware::store_failure()
    };
    let mut expected_headers = expected.headers().clone();
    let expected_body = to_bytes(expected.into_body(), 4096).await.unwrap();
    // Axum supplies Content-Length when the response passes through Router.
    expected_headers.insert(
        header::CONTENT_LENGTH,
        expected_body.len().to_string().parse().unwrap(),
    );
    assert_eq!(response.headers(), &expected_headers);
    assert_eq!(
        to_bytes(response.into_body(), 4096).await.unwrap(),
        expected_body
    );
}

#[tokio::test]
async fn final_store_failure_is_attributed_without_changing_public_response() {
    let f = Fixture::new().await;
    f.store.set_unavailable(true);
    let app = app(f.auth.clone());
    let (response, events) = capture(app.clone().oneshot(request(
        "must-not-leak-store-request",
        true,
        "/api/v1/admin/usage/metrics/view",
    )))
    .await;
    assert_failure(response.unwrap(), None).await;
    assert_eq!(events.len(), 1);
    // The in-memory store does not annotate a dependency. Do not guess a cause.
    assert_eq!(
        events[0]["fields"]["dependency"],
        "authentication_authority"
    );
    assert_eq!(events[0]["fields"]["reason"], "unknown");
    f.store.set_unavailable(false);
    let (response, events) = capture(
        app.oneshot(
            Request::get("/api/v1/admin/usage/metrics/view")
                .body(Body::empty())
                .unwrap(),
        ),
    )
    .await;
    assert_eq!(response.unwrap().status(), StatusCode::OK);
    assert!(events.is_empty());
}

#[tokio::test]
async fn shared_provider_failure_keeps_each_callers_request_hash() {
    let f = Fixture::new().await;
    f.auth.metadata().await.unwrap();
    let gate = Arc::new(Gate::default());
    let introspection_gate = Arc::new(Gate::default());
    f.control.introspection.write().await.gate = Some(introspection_gate.clone());
    let mut reply = f.control.userinfo.write().await;
    reply.body = "must-not-leak-invalid-json".into();
    reply.gate = Some(gate.clone());
    drop(reply);
    let app = app(f.auth.clone());
    let before = f.control.requests.load(Ordering::SeqCst);
    let ((first, second, ()), events) = capture(async {
        tokio::join!(
            app.clone().oneshot(request(
                "must-not-leak-first",
                false,
                "/api/v1/admin/usage/metrics/view"
            )),
            app.clone().oneshot(request(
                "must-not-leak-second",
                false,
                "/api/v1/admin/usage/metrics/view"
            )),
            async {
                gate.reached.notified().await;
                introspection_gate.reached.notified().await;
                gate.release.notify_one();
                introspection_gate.release.notify_one();
            }
        )
    })
    .await;
    assert_failure(first.unwrap(), None).await;
    assert_failure(second.unwrap(), None).await;
    assert_eq!(f.control.requests.load(Ordering::SeqCst) - before, 2);
    assert_eq!(events.len(), 2);
    for event in &events {
        assert_eq!(event["fields"]["dependency"], "identity_provider");
        assert_eq!(event["fields"]["reason"], "invalid_evidence");
    }
    assert_ne!(
        events[0]["fields"]["request_hash"],
        events[1]["fields"]["request_hash"]
    );
    // The same request ID maps to the same process-salted hash, independently
    // of which caller led the provider flight.
    f.control.userinfo.write().await.gate = None;
    f.control.introspection.write().await.gate = None;
    let (_, repeated) = capture(app.oneshot(request(
        "must-not-leak-first",
        false,
        "/api/v1/admin/usage/metrics/view",
    )))
    .await;
    assert_eq!(repeated.len(), 1);
    assert!(events
        .iter()
        .any(|event| event["fields"]["request_hash"] == repeated[0]["fields"]["request_hash"]));
}

#[tokio::test]
async fn final_throttle_keeps_retry_headers_and_success_emits_no_failure() {
    let f = Fixture::new().await;
    f.auth.metadata().await.unwrap();
    let app = app(f.auth.clone());
    f.control.introspection.write().await.status = StatusCode::TOO_MANY_REQUESTS;
    f.control
        .introspection
        .write()
        .await
        .headers
        .insert(header::RETRY_AFTER, "3".parse().unwrap());
    let (response, events) = capture(app.clone().oneshot(request(
        "must-not-leak-throttle",
        false,
        "/auth/session",
    )))
    .await;
    assert_failure(response.unwrap(), Some("3")).await;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["fields"]["reason"], "http_429");
    f.control.introspection.write().await.status = StatusCode::OK;
    // A completed throttle remains authoritative until its backoff expires.
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(3)).await;
    tokio::time::resume();
    let (response, events) = capture(app.oneshot(request(
        "must-not-leak-success",
        false,
        "/api/v1/admin/usage/metrics/view",
    )))
    .await;
    assert_eq!(response.unwrap().status(), StatusCode::OK);
    assert!(events.is_empty());
}

#[tokio::test]
async fn recovered_provider_attempt_emits_no_final_failure() {
    let f = Fixture::new().await;
    f.auth.metadata().await.unwrap();
    let gate = Arc::new(Gate::default());
    let mut reply = f.control.userinfo.write().await;
    reply.status = StatusCode::SERVICE_UNAVAILABLE;
    reply.gate = Some(gate.clone());
    drop(reply);
    let before = f.control.requests.load(Ordering::SeqCst);
    let ((response, ()), events) = capture(async {
        tokio::join!(
            app(f.auth.clone()).oneshot(request(
                "must-not-leak-recovered",
                false,
                "/api/v1/admin/usage/metrics/view"
            )),
            async {
                gate.reached.notified().await;
                let mut reply = f.control.userinfo.write().await;
                reply.status = StatusCode::OK;
                reply.gate = None;
                drop(reply);
                gate.release.notify_one();
            }
        )
    })
    .await;
    assert_eq!(response.unwrap().status(), StatusCode::OK);
    assert_eq!(f.control.requests.load(Ordering::SeqCst) - before, 4);
    assert!(events.is_empty());
}
