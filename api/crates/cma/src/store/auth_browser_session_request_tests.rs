//! End-to-end identity-chain budgets and local wait-amplification measurements.
use super::*;
use crate::auth::{persistence::test_observer::ObservedAuthStore, AuthMode, AuthState};
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
                    async { Json(json!({"sub":"fixture-owner", "permissions":["cma:*"]})) }
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
                let expected = match (joined, path == "/auth/session") {
                    (false, false) => vec!["find", "find", "find"],
                    (false, true) => vec!["find", "find", "find", "find"],
                    (true, false) => vec!["aggregate", "find"],
                    (true, true) => vec!["aggregate", "find", "find"],
                };
                fixture.take_commands();
                *fixture.measurements.lock().unwrap() = Measurements::default();
                provider.before_provider.lock().unwrap().clear();
                provider
                    .requests
                    .store(0, std::sync::atomic::Ordering::Relaxed);
                let mut elapsed = Vec::new();
                for _ in 0..SAMPLES {
                    let start = Instant::now();
                    *provider.started.lock().unwrap() = Some(start);
                    assert_eq!(request(&app, path).await.status(), StatusCode::OK);
                    elapsed.push(start.elapsed().as_secs_f64() * 1000.);
                    assert_eq!(fixture.take_commands(), expected);
                }
                *provider.started.lock().unwrap() = None;
                let mut before = provider.before_provider.lock().unwrap().clone();
                assert_eq!(before.len(), SAMPLES);
                assert_eq!(
                    provider.requests.load(std::sync::atomic::Ordering::Relaxed),
                    SAMPLES * 2
                );
                assert!(observed
                    .reads
                    .lock()
                    .unwrap()
                    .iter()
                    .all(|(_, phase)| *phase == "identity"));
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
