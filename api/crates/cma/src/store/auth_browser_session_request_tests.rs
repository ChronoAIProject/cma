//! End-to-end identity-chain budgets and local wait-amplification measurements.
use super::*;
use crate::auth::{
    persistence::test_observer::{ObservedAuthStore, ReadGate},
    AuthMode, AuthState,
};
use axum::{
    body::Body,
    http::{header, Request, StatusCode},
    routing::{get, post},
    Json, Router,
};
use serde_json::json;
use std::time::Instant;
use tower::ServiceExt;

struct Provider {
    base: String,
    server: tokio::task::JoinHandle<()>,
    started: Arc<Mutex<Option<Instant>>>,
    before_provider: Arc<Mutex<Vec<f64>>>,
    requests: Arc<std::sync::atomic::AtomicUsize>,
    gate: Arc<Mutex<Option<Arc<ReadGate>>>>,
}
impl Drop for Provider {
    fn drop(&mut self) {
        self.server.abort();
    }
}
impl Provider {
    async fn new() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let metadata = json!({"authorization_endpoint": format!("{base}/authorize"),
            "token_endpoint": format!("{base}/token"), "userinfo_endpoint":format!("{base}/userinfo"),
            "introspection_endpoint":format!("{base}/introspect")});
        let started: Arc<Mutex<Option<Instant>>> = Arc::new(Mutex::new(None));
        let before_provider = Arc::new(Mutex::new(Vec::new()));
        let requests = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let gate = Arc::new(Mutex::new(None::<Arc<ReadGate>>));
        let verification_gate = gate.clone();
        let (clock, times, info_count, introspect_count) = (
            started.clone(),
            before_provider.clone(),
            requests.clone(),
            requests.clone(),
        );
        let app = Router::new()
            .route(
                "/.well-known/openid-configuration",
                get(move || async move { Json(metadata) }),
            )
            .route(
                "/userinfo",
                get(move || {
                    if let Some(start) = *clock.lock().unwrap() {
                        times
                            .lock()
                            .unwrap()
                            .push(start.elapsed().as_secs_f64() * 1000.);
                    }
                    info_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let gate = verification_gate.lock().unwrap().clone();
                    async move {
                        if let Some(gate) = gate {
                            gate.reached.notify_one();
                            gate.release.notified().await;
                        }
                        Json(json!({"sub":"fixture-owner", "permissions":["cma:*"]}))
                    }
                }),
            )
            .route(
                "/introspect",
                post(move || {
                    introspect_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    async {
                        Json(json!({"active":true, "sub":"fixture-owner", "permissions":["cma:*"]}))
                    }
                }),
            );
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self {
            base,
            server,
            started,
            before_provider,
            requests,
            gate,
        }
    }

    fn auth(&self, store: Arc<dyn AuthStore>, joined: bool) -> Arc<AuthState> {
        AuthState::with_store(
            AuthMode::Provider {
                base_url: self.base.clone(),
                client_id: "fixture-client".into(),
                client_secret: "fixture-secret".into(),
                public_url: "https://example.test".into(),
            },
            None,
            store,
        )
        .with_browser_session_join(joined)
    }
}

fn app(auth: Arc<AuthState>) -> Router {
    Router::new()
        .merge(crate::auth::router(auth.clone()))
        .route(
            "/api/v2/ui/task-experience",
            get(|_: layer1_public::AuthenticatedCaller| async { StatusCode::OK }),
        )
        .layer(axum::middleware::from_fn_with_state(
            auth,
            crate::auth::attach_subject,
        ))
}

async fn request(app: &Router, path: &str) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::get(path)
                .header(header::COOKIE, "cma_session=fixture-cookie")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

fn percentile(values: &mut [f64], percentile: usize) -> f64 {
    values.sort_by(f64::total_cmp);
    values[(values.len() * percentile).div_ceil(100) - 1]
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn cookie_request_chain_commands_and_local_wait_measurements() {
    let fixture = Fixture::new().await;
    let provider = Provider::new().await;
    let mut login = login();
    login.id = SecretDigest::identify("browser-login", "fixture-cookie");
    fixture.store.put_browser_login(login).await.unwrap();
    fixture.store.insert_subject(state()).await.unwrap();
    const SAMPLES: usize = 50;
    for wait_ms in [0, 25] {
        for joined in [false, true] {
            let observed = Arc::new(ObservedAuthStore::new(
                fixture.store.clone(),
                Duration::from_millis(wait_ms),
            ));
            let app = app(provider.auth(observed.clone(), joined));
            // Discovery, TLS-independent HTTP connection setup and Mongo warmup are excluded.
            assert_eq!(
                request(&app, "/auth/session").await.status(),
                StatusCode::OK
            );
            for path in ["/api/v2/ui/task-experience", "/auth/session"] {
                // #2001 rechecks login after provider/custody IO on every route.
                // This fixture has no platform-settings service, so /auth/session
                // adds no handler Mongo read to either identity chain.
                let (expected, expected_reads) = if joined {
                    (
                        vec!["aggregate", "find", "find"],
                        vec![
                            ("join", "identity"),
                            ("subject", "identity"),
                            ("login", "identity"),
                        ],
                    )
                } else {
                    (
                        vec!["find", "find", "find", "find"],
                        vec![
                            ("login", "identity"),
                            ("subject", "identity"),
                            ("subject", "identity"),
                            ("login", "identity"),
                        ],
                    )
                };
                fixture.take_commands();
                *fixture.measurements.lock().unwrap() = Measurements::default();
                provider.before_provider.lock().unwrap().clear();
                provider
                    .requests
                    .store(0, std::sync::atomic::Ordering::Relaxed);
                let mut elapsed = Vec::new();
                for sample in 0..SAMPLES {
                    observed.reads.lock().unwrap().clear();
                    let start = Instant::now();
                    *provider.started.lock().unwrap() = Some(start);
                    assert_eq!(request(&app, path).await.status(), StatusCode::OK);
                    let elapsed_ms = start.elapsed().as_secs_f64() * 1000.;
                    assert!(elapsed_ms >= wait_ms as f64 * expected_reads.len() as f64);
                    elapsed.push(elapsed_ms);
                    assert_eq!(
                        fixture.take_commands(),
                        expected,
                        "joined={joined} path={path} sample={sample}"
                    );
                    assert_eq!(
                        std::mem::take(&mut *observed.reads.lock().unwrap()),
                        expected_reads,
                        "joined={joined} path={path} sample={sample}"
                    );
                }
                *provider.started.lock().unwrap() = None;
                let mut before = provider.before_provider.lock().unwrap().clone();
                assert_eq!(before.len(), SAMPLES);
                assert_eq!(
                    provider.requests.load(std::sync::atomic::Ordering::Relaxed),
                    SAMPLES * 2
                );
                let initial_reads = if joined { 1 } else { 2 };
                assert!(before
                    .iter()
                    .all(|ms| *ms >= (wait_ms * initial_reads) as f64));
                let stats = fixture.measurements.lock().unwrap();
                assert_eq!(stats.checkouts, SAMPLES * expected.len());
                println!("identity_mongo joined={joined} simulated_wait_ms={wait_ms} path={path} round_trips={} reply_bson_bytes={} command_total_us={} actual_checkout_mean_us={:.3}",
                    stats.checkouts, stats.reply_bytes, stats.command_us, stats.checkout_us as f64 / stats.checkouts as f64);
                println!("identity_local joined={joined} path={path} samples={SAMPLES} simulated_wait_ms={wait_ms} commands_per_request={} request_p50_ms={:.3} request_p95_ms={:.3} before_provider_p50_ms={:.3} before_provider_p95_ms={:.3}",
                    expected.len(), percentile(&mut elapsed, 50), percentile(&mut elapsed, 95),
                    percentile(&mut before, 50), percentile(&mut before, 95));
            }
        }
    }
    fixture.close().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn joined_session_returns_no_authority_after_revocation() {
    let fixture = Fixture::new().await;
    let provider = Provider::new().await;
    let mut login = login();
    login.id = SecretDigest::identify("browser-login", "fixture-cookie");
    fixture
        .store
        .put_browser_login(login.clone())
        .await
        .unwrap();
    fixture.store.insert_subject(state()).await.unwrap();
    let app = app(provider.auth(fixture.store.clone(), true));
    assert_eq!(
        request(&app, "/auth/session").await.status(),
        StatusCode::OK
    );
    fixture.store.delete_browser_login(&login.id).await.unwrap();
    assert_eq!(
        request(&app, "/auth/session").await.status(),
        StatusCode::UNAUTHORIZED
    );
    fixture.close().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn logout_during_provider_verification_refuses_both_cookie_routes() {
    for joined in [false, true] {
        for path in ["/api/v2/ui/task-experience", "/auth/session"] {
            let fixture = Fixture::new().await;
            let provider = Provider::new().await;
            let mut login = login();
            login.id = SecretDigest::identify("browser-login", "fixture-cookie");
            fixture
                .store
                .put_browser_login(login.clone())
                .await
                .unwrap();
            fixture.store.insert_subject(state()).await.unwrap();
            let gate = Arc::new(ReadGate::default());
            *provider.gate.lock().unwrap() = Some(gate.clone());
            let app = app(provider.auth(fixture.store.clone(), joined));
            let pending = tokio::spawn(async move { request(&app, path).await });
            tokio::time::timeout(Duration::from_secs(5), gate.reached.notified())
                .await
                .expect("request reaches provider after selecting durable login");
            // Revocation after the initial read must win over successful provider
            // evidence and unchanged subject custody on both joined/point paths.
            fixture.store.delete_browser_login(&login.id).await.unwrap();
            gate.release.notify_one();
            let response = pending.await.unwrap();
            assert_eq!(
                response.status(),
                StatusCode::UNAUTHORIZED,
                "joined={joined} path={path}"
            );
            fixture.close().await;
        }
    }
}
