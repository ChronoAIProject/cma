use super::*;
use layer2_core::dependency_diagnostics::{capture, Dependency, Failure, Reason};
use layer2_core::diagnostic_context::DiagnosticContext;

const TOKEN: &str = "must-not-leak-access-token";

async fn request(
    f: &Fixture,
    state: &SubjectAuthState,
    cookie: bool,
) -> Result<Option<RequestAuth>, StoreError> {
    if cookie {
        request_for_browser_cookie(&f.auth, &state.subject, state.clone()).await
    } else {
        identity::request_for_bearer(&f.auth, TOKEN.into()).await
    }
}

async fn local_capacity_survives_cookie_and_bearer_public_projection(cookie: bool) {
    let f = Fixture::new().await;
    let state = f.seed().await;
    f.auth.metadata().await.unwrap();
    let permits = f
        .auth
        .fixture_verification_slots()
        .acquire_many(64)
        .await
        .unwrap();
    let before = f.control.requests.load(Ordering::SeqCst);
    let (result, failure) = capture(request(&f, &state, cookie)).await;
    assert!(matches!(result, Err(StoreError::Unavailable(_))));
    assert_eq!(f.control.requests.load(Ordering::SeqCst), before);
    assert_eq!(
        failure,
        Some(Failure::new(Dependency::Local, Reason::Capacity))
    );
    drop(permits);
    let (result, failure) = capture(request(&f, &state, cookie)).await;
    assert!(result.unwrap().is_some());
    assert_eq!(failure, None);
    assert_eq!(f.control.requests.load(Ordering::SeqCst), before + 2);
    f.assert_unchanged(&state).await;
}

#[tokio::test]
async fn cookie_local_capacity_survives_cookie_and_bearer_public_projection() {
    local_capacity_survives_cookie_and_bearer_public_projection(true).await;
}

#[tokio::test]
async fn bearer_local_capacity_survives_cookie_and_bearer_public_projection() {
    local_capacity_survives_cookie_and_bearer_public_projection(false).await;
}

async fn local_fence_capacity_survives_cookie_and_bearer_public_projection(cookie: bool) {
    let f = Fixture::new().await;
    let state = f.seed().await;
    let mut fences = Vec::new();
    for index in 0..1024 {
        fences.push(
            f.auth
                .fixture_token_epoch(&format!("synthetic-fence-{index}"))
                .unwrap(),
        );
    }
    let before = f.control.requests.load(Ordering::SeqCst);
    let (result, failure) = capture(request(&f, &state, cookie)).await;
    assert!(matches!(result, Err(StoreError::Unavailable(_))));
    assert_eq!(f.control.requests.load(Ordering::SeqCst), before);
    assert_eq!(f.auth.fixture_verification_slots().available_permits(), 64);
    assert_eq!(
        failure,
        Some(Failure::new(Dependency::Local, Reason::Capacity))
    );
    drop(fences.pop());
    assert_eq!(fences.len(), 1023);
    let (result, failure) = capture(request(&f, &state, cookie)).await;
    assert!(result.unwrap().is_some());
    assert_eq!(failure, None);
}

#[tokio::test]
async fn cookie_local_fence_capacity_survives_cookie_and_bearer_public_projection() {
    local_fence_capacity_survives_cookie_and_bearer_public_projection(true).await;
}

#[tokio::test]
async fn bearer_local_fence_capacity_survives_cookie_and_bearer_public_projection() {
    local_fence_capacity_survives_cookie_and_bearer_public_projection(false).await;
}

async fn caller(
    f: &Fixture,
    state: &SubjectAuthState,
    cookie: bool,
    hash: &'static str,
) -> (
    Result<Option<RequestAuth>, StoreError>,
    Option<Failure>,
    Arc<str>,
) {
    DiagnosticContext {
        request_hash: hash.into(),
        ..DiagnosticContext::default()
    }
    .scope(async {
        let (result, failure) = capture(request(f, state, cookie)).await;
        (result, failure, DiagnosticContext::current().request_hash)
    })
    .await
}

async fn shared_failure_is_retained_in_each_callers_own_context(cookie: bool, malformed: bool) {
    let f = Fixture::new().await;
    let state = f.seed().await;
    f.auth.metadata().await.unwrap();
    let before = f.control.requests.load(Ordering::SeqCst);
    let gate = Arc::new(Gate::default());
    f.control.introspection.write().await.gate = Some(gate.clone());
    let normal_body = f.control.introspection.read().await.body.clone();
    if malformed {
        f.control.introspection.write().await.body = "synthetic-invalid-provider-body".into();
    }
    let first = caller(&f, &state, cookie, "leader-hash");
    let second = caller(&f, &state, cookie, "waiter-hash");
    tokio::pin!(first, second);
    assert!(futures_util::poll!(first.as_mut()).is_pending());
    tokio::select! {
        _ = &mut first => panic!("leader completed before the provider barrier"),
        reached = tokio::time::timeout(Duration::from_secs(2), gate.reached.notified()) => reached.unwrap(),
    }
    assert!(futures_util::poll!(second.as_mut()).is_pending());
    if !malformed {
        f.auth.invalidate_fixture_token(TOKEN);
    }
    gate.release.notify_one();
    let (first, second) = tokio::join!(first, second);
    for ((result, failure, hash), expected) in [(first, "leader-hash"), (second, "waiter-hash")] {
        assert!(matches!(result, Err(StoreError::Unavailable(_))));
        assert_eq!(hash.as_ref(), expected);
        assert_eq!(
            failure,
            Some(Failure::new(
                Dependency::IdentityProvider,
                Reason::InvalidEvidence
            ))
        );
    }
    assert_eq!(f.control.requests.load(Ordering::SeqCst), before + 2);
    f.control.introspection.write().await.gate = None;
    f.control.introspection.write().await.body = normal_body;
    let (result, failure) = capture(request(&f, &state, cookie)).await;
    assert!(result.unwrap().is_some());
    assert_eq!(failure, None);
    assert_eq!(f.control.requests.load(Ordering::SeqCst), before + 4);
}

#[tokio::test]
async fn cookie_shared_epoch_failure_is_retained_in_each_callers_own_context() {
    shared_failure_is_retained_in_each_callers_own_context(true, false).await;
}

#[tokio::test]
async fn bearer_shared_epoch_failure_is_retained_in_each_callers_own_context() {
    shared_failure_is_retained_in_each_callers_own_context(false, false).await;
}

#[tokio::test]
async fn shorter_cookie_waiter_deadline_does_not_poison_the_leader() {
    let f = Fixture::new().await;
    let state = f.seed().await;
    let gate = Arc::new(Gate::default());
    f.auth.metadata().await.unwrap();
    f.control.introspection.write().await.gate = Some(gate.clone());
    let leader = caller(&f, &state, true, "leader-hash");
    tokio::pin!(leader);
    assert!(futures_util::poll!(leader.as_mut()).is_pending());
    tokio::select! {
        _ = &mut leader => panic!("leader completed before the provider barrier"),
        reached = tokio::time::timeout(Duration::from_secs(2), gate.reached.notified()) => reached.unwrap(),
    }
    let (result, failure) = capture(request_with_timeout(
        &f.auth,
        &state.subject,
        state.clone(),
        Duration::ZERO,
    ))
    .await;
    assert_unavailable(result);
    assert_eq!(
        failure,
        Some(Failure::new(
            Dependency::IdentityProvider,
            Reason::InvalidEvidence
        ))
    );
    gate.release.notify_one();
    let (result, failure, hash) = leader.await;
    assert!(result.unwrap().is_some());
    assert_eq!(failure, None);
    assert_eq!(hash.as_ref(), "leader-hash");
    assert_eq!(f.auth.fixture_verification_slots().available_permits(), 64);
}

#[tokio::test]
async fn cookie_shared_malformed_provider_body_keeps_each_callers_provider_attribution() {
    shared_failure_is_retained_in_each_callers_own_context(true, true).await;
}

#[tokio::test]
async fn bearer_shared_malformed_provider_body_keeps_each_callers_provider_attribution() {
    shared_failure_is_retained_in_each_callers_own_context(false, true).await;
}

#[tokio::test]
async fn capacity_keeps_public_refusal_and_emits_one_local_counter_per_failed_caller() {
    use axum::{
        body::{to_bytes, Body},
        http::Request,
    };
    use tower::ServiceExt as _;
    use tracing::{instrument::WithSubscriber, Instrument};
    #[derive(Clone, Default)]
    struct Writer(Arc<std::sync::Mutex<Vec<u8>>>);
    impl std::io::Write for Writer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let output = Writer::default();
    let writer = output.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .event_format(crate::diagnostic_format::text())
        .with_max_level(tracing::Level::INFO)
        .with_writer(move || writer.clone())
        .finish();
    async {
        for cookie in [true, false] {
            let f = Fixture::new().await;
            let state = f.seed().await;
            f.auth.metadata().await.unwrap();
            let mut headers = HeaderMap::new();
            if cookie {
                let login = f
                    .auth
                    .issue_browser_login(state.subject.clone())
                    .await
                    .unwrap();
                headers.insert(
                    header::COOKIE,
                    format!("cma_session={login}").parse().unwrap(),
                );
            } else {
                headers.insert(
                    header::AUTHORIZATION,
                    format!("Bearer {TOKEN}").parse().unwrap(),
                );
            }
            let calls = Arc::new(AtomicUsize::new(0));
            let handler_calls = calls.clone();
            let app = Router::new()
                .route(
                    "/protected",
                    get(move || {
                        handler_calls.fetch_add(1, Ordering::SeqCst);
                        async { StatusCode::OK }
                    }),
                )
                .layer(axum::middleware::from_fn_with_state(
                    f.auth.clone(),
                    crate::auth::attach_subject,
                ));
            let permits = f
                .auth
                .fixture_verification_slots()
                .acquire_many(64)
                .await
                .unwrap();
            let before = f.control.requests.load(Ordering::SeqCst);
            let mut request = Request::get("/protected").body(Body::empty()).unwrap();
            *request.headers_mut() = headers.clone();
            let response = app.clone().oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
            assert!(!response.headers().contains_key(header::RETRY_AFTER));
            let body: Value =
                serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap())
                    .unwrap();
            assert_eq!(body["code"], "store_failed");
            assert_eq!(body["retryable"], false);
            assert_eq!(calls.load(Ordering::SeqCst), 0);
            assert_eq!(f.control.requests.load(Ordering::SeqCst), before);
            drop(permits);
            let mut request = Request::get("/protected").body(Body::empty()).unwrap();
            *request.headers_mut() = headers;
            assert_eq!(app.oneshot(request).await.unwrap().status(), StatusCode::OK);
            assert_eq!(calls.load(Ordering::SeqCst), 1);
        }
    }
    .instrument(tracing::info_span!(
        "private-parent",
        secret = "must-not-leak-span"
    ))
    .with_subscriber(subscriber)
    .await;
    let text = String::from_utf8(output.0.lock().unwrap().clone()).unwrap();
    let events: Vec<_> = text
        .lines()
        .filter(|event| event.contains("cma_dependency_failures_total"))
        .collect();
    assert_eq!(events.len(), 2);
    for event in events {
        assert!(event.contains("dependency=\"local\""), "{event}");
        assert!(event.contains("reason=\"capacity\""), "{event}");
        for private in [
            "must-not-leak",
            "synthetic",
            "owner",
            "http://",
            "private-parent",
        ] {
            assert!(!event.contains(private), "{event}");
        }
    }
}
