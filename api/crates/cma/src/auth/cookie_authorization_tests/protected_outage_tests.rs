use super::*;
use axum::{
    body::{to_bytes, Body},
    http::{Method, Request},
    middleware::Next,
};
use layer1_public::AuthenticatedCaller;
use tower::ServiceExt as _;

const SANDBOX: &str = "/api/v1/sandboxes/fixture";
const WORKSPACE: &str = "/api/v1/workspaces/fixture";
pub(super) const READS: [&str; 3] = ["/auth/session", SANDBOX, WORKSPACE];

pub(super) fn application(auth: Arc<AuthState>, calls: Arc<AtomicUsize>) -> Router {
    async fn protected(AuthenticatedCaller(caller): AuthenticatedCaller) -> StatusCode {
        if caller.authorization.allows(Capability::UseOwnSandboxes) {
            StatusCode::OK
        } else {
            StatusCode::FORBIDDEN
        }
    }
    crate::auth::router(auth.clone())
        .route(
            SANDBOX,
            get(protected)
                .post(protected)
                .patch(protected)
                .delete(protected),
        )
        .route(WORKSPACE, get(protected))
        .layer(axum::middleware::from_fn(
            move |request: Request<Body>, next: Next| {
                let calls = calls.clone();
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    next.run(request).await
                }
            },
        ))
        .layer(axum::middleware::from_fn_with_state(
            auth,
            crate::auth::attach_subject,
        ))
}

pub(super) async fn credentials(fixture: &Fixture, cookie: bool) -> HeaderMap {
    let mut headers = HeaderMap::new();
    if cookie {
        let login = fixture
            .auth
            .issue_browser_login(SubjectId::new("owner"))
            .await
            .unwrap();
        headers.insert(
            header::COOKIE,
            format!("cma_session={login}").parse().unwrap(),
        );
    } else {
        headers.insert(
            header::AUTHORIZATION,
            "Bearer independent-fixture-token".parse().unwrap(),
        );
    }
    headers
}

pub(super) async fn request(
    app: &Router,
    credentials: &HeaderMap,
    method: Method,
    path: &str,
) -> (StatusCode, HeaderMap, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .body(Body::empty())
        .unwrap();
    *request.headers_mut() = credentials.clone();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = to_bytes(response.into_body(), 65536).await.unwrap();
    let text = String::from_utf8_lossy(&bytes);
    for private in [
        "fixture-token",
        "must-not-leak",
        "Stored Owner",
        "Live Owner",
    ] {
        if status == StatusCode::SERVICE_UNAVAILABLE {
            assert!(!text.contains(private));
        }
    }
    (
        status,
        headers,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn assert_transient(response: (StatusCode, HeaderMap, Value), delay: u64) {
    let (status, headers, body) = response;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(headers[header::CONTENT_TYPE], "application/problem+json");
    assert_eq!(headers[header::CACHE_CONTROL], "no-store");
    let after: u64 = headers[header::RETRY_AFTER]
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    assert!((1..=delay).contains(&after));
    assert_eq!(body["code"], "identity_unavailable");
    assert_eq!(body["retryable"], true);
    assert_eq!(body["status"], 503);
}

#[tokio::test]
async fn protected_reads_preserve_classified_timeouts_and_upstream_outages_then_reverify() {
    for cookie in [false, true] {
        for fault in ["headers_timeout", "http_502", "http_503"] {
            let mut fixture = Fixture::new().await;
            Arc::get_mut(&mut fixture.auth).unwrap().http = reqwest::Client::builder()
                .timeout(Duration::from_millis(200))
                .build()
                .unwrap();
            let retained = fixture.seed().await;
            fixture.auth.metadata().await.unwrap();
            let credentials = credentials(&fixture, cookie).await;
            let calls = Arc::new(AtomicUsize::new(0));
            let app = application(fixture.auth.clone(), calls.clone());
            let original = fixture.control.userinfo.read().await.clone();
            {
                let mut reply = fixture.control.userinfo.write().await;
                match fault {
                    "headers_timeout" => reply.delay = Duration::from_secs(1),
                    "http_502" => reply.status = StatusCode::BAD_GATEWAY,
                    _ => reply.status = StatusCode::SERVICE_UNAVAILABLE,
                }
            }
            for path in READS {
                let before = fixture.control.requests.load(Ordering::SeqCst);
                assert_transient(request(&app, &credentials, Method::GET, path).await, 3);
                let attempted = fixture.control.requests.load(Ordering::SeqCst) - before;
                assert!(
                    (1..=6).contains(&attempted),
                    "existing three-attempt budget: {attempted}"
                );
                assert_eq!(calls.load(Ordering::SeqCst), 0, "{fault} {path}");
            }
            *fixture.control.userinfo.write().await = original;
            for path in READS {
                let before = fixture.control.requests.load(Ordering::SeqCst);
                assert_eq!(
                    request(&app, &credentials, Method::GET, path).await.0,
                    StatusCode::OK
                );
                assert_eq!(fixture.control.requests.load(Ordering::SeqCst) - before, 2);
            }
            assert_eq!(calls.load(Ordering::SeqCst), READS.len());
            fixture.assert_unchanged(&retained).await;
            assert_eq!(fixture.control.exchange_requests.load(Ordering::SeqCst), 0);
        }
    }
}

#[tokio::test]
async fn malformed_partial_and_unknown_failures_never_gain_retry_permission() {
    for cookie in [false, true] {
        for fault in [
            "malformed",
            "partial",
            "body_timeout",
            "unknown_status",
            "store",
        ] {
            if fault == "store" && !cookie {
                continue;
            }
            let mut fixture = Fixture::new().await;
            Arc::get_mut(&mut fixture.auth).unwrap().http = reqwest::Client::builder()
                .timeout(Duration::from_millis(200))
                .build()
                .unwrap();
            fixture.seed().await;
            fixture.auth.metadata().await.unwrap();
            let credentials = credentials(&fixture, cookie).await;
            let calls = Arc::new(AtomicUsize::new(0));
            let app = application(fixture.auth.clone(), calls.clone());
            {
                let mut reply = fixture.control.userinfo.write().await;
                match fault {
                    "malformed" => reply.body = "{".into(),
                    "partial" => {
                        reply
                            .headers
                            .insert(header::CONTENT_RANGE, "bytes 0-1/10".parse().unwrap());
                    }
                    "body_timeout" => {
                        reply.body = "{".into();
                        reply.stream_tail = Some((Duration::from_secs(1), Ok("}".into())));
                    }
                    "unknown_status" => reply.status = StatusCode::INTERNAL_SERVER_ERROR,
                    _ => fixture.store.set_unavailable(true),
                }
            }
            for path in READS {
                use super::protected_outage_counts::{dispatches, measured};
                let ((status, headers, body), metrics) =
                    measured(request(&app, &credentials, Method::GET, path)).await;
                assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{fault} {path}");
                assert_eq!(body["code"], "store_failed");
                assert_eq!(body["retryable"], false);
                assert!(!headers.contains_key(header::RETRY_AFTER));
                let attempted = dispatches(&metrics);
                assert!(
                    attempted <= 2,
                    "cookie={cookie} fault={fault} path={path} dispatches={attempted}\n{metrics}"
                );
                assert_eq!(attempted, if fault == "store" { 0 } else { 2 });
                assert_eq!(calls.load(Ordering::SeqCst), 0);
            }
        }
    }
}

#[tokio::test]
async fn protected_reads_keep_rate_limit_deadlines_without_admitting_handlers() {
    let fixture = Fixture::new().await;
    fixture.seed().await;
    fixture.auth.metadata().await.unwrap();
    let credentials = credentials(&fixture, true).await;
    let calls = Arc::new(AtomicUsize::new(0));
    let app = application(fixture.auth.clone(), calls.clone());
    for endpoint in [&fixture.control.userinfo, &fixture.control.introspection] {
        let mut reply = endpoint.write().await;
        reply.status = StatusCode::TOO_MANY_REQUESTS;
        reply
            .headers
            .insert(header::RETRY_AFTER, "60".parse().unwrap());
    }
    for path in READS {
        assert_transient(request(&app, &credentials, Method::GET, path).await, 60);
    }
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let before = fixture.control.requests.load(Ordering::SeqCst);
    for endpoint in [&fixture.control.userinfo, &fixture.control.introspection] {
        endpoint.write().await.status = StatusCode::OK;
    }
    for path in READS {
        assert_transient(request(&app, &credentials, Method::GET, path).await, 60);
    }
    assert_eq!(fixture.control.requests.load(Ordering::SeqCst), before);
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(61)).await;
    tokio::time::resume();
    assert_eq!(
        request(&app, &credentials, Method::GET, SANDBOX).await.0,
        StatusCode::OK
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn two_replicas_reject_outages_and_revocation_without_reusing_prior_success() {
    for cookie in [false, true] {
        let fixture = Fixture::new().await;
        fixture.seed().await;
        let credentials = credentials(&fixture, cookie).await;
        let second = AuthState::with_store(fixture.auth.mode.clone(), None, fixture.store.clone());
        let calls = Arc::new(AtomicUsize::new(0));
        let apps = [
            application(fixture.auth.clone(), calls.clone()),
            application(second, calls.clone()),
        ];
        for app in &apps {
            assert_eq!(
                request(app, &credentials, Method::GET, SANDBOX).await.0,
                StatusCode::OK
            );
        }
        calls.store(0, Ordering::SeqCst);
        fixture.control.userinfo.write().await.status = StatusCode::SERVICE_UNAVAILABLE;
        for app in &apps {
            assert_transient(request(app, &credentials, Method::GET, WORKSPACE).await, 3);
            for method in [Method::POST, Method::PATCH, Method::DELETE] {
                let (status, _, body) = request(app, &credentials, method, SANDBOX).await;
                assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
                assert_eq!(body["code"], "store_failed");
                assert_eq!(body["retryable"], false);
            }
        }
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        for status in [StatusCode::UNAUTHORIZED, StatusCode::FORBIDDEN] {
            fixture.control.userinfo.write().await.status = status;
            for app in &apps {
                assert_eq!(
                    request(app, &credentials, Method::GET, WORKSPACE).await.0,
                    StatusCode::UNAUTHORIZED
                );
            }
        }
        fixture.control.userinfo.write().await.status = StatusCode::OK;
        *fixture.control.introspection.write().await =
            Reply::json(json!({"active": true, "sub": "owner", "permissions": []}));
        for app in &apps {
            assert_eq!(
                request(app, &credentials, Method::GET, WORKSPACE).await.0,
                StatusCode::FORBIDDEN
            );
        }
    }
}
