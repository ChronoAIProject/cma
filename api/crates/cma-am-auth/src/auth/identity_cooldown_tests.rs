use super::*;
use axum::{
    extract::{Request, State},
    http::{header, StatusCode},
    response::IntoResponse,
    Router,
};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::Notify;

type RequestScope = (String, String);

#[derive(Clone)]
struct Reply {
    status: StatusCode,
    after: &'static str,
}

struct Provider {
    replies: Mutex<HashMap<RequestScope, Reply>>,
    calls: Mutex<HashMap<RequestScope, usize>>,
    total: AtomicUsize,
    blocked: AtomicBool,
    reached: Notify,
    release: Notify,
}

async fn respond(State(state): State<Arc<Provider>>, request: Request) -> impl IntoResponse {
    let key = (
        request.uri().path().to_owned(),
        request.headers()[header::AUTHORIZATION]
            .to_str()
            .unwrap()
            .to_owned(),
    );
    *state.calls.lock().unwrap().entry(key.clone()).or_default() += 1;
    state.total.fetch_add(1, Ordering::SeqCst);
    let reply = state
        .replies
        .lock()
        .unwrap()
        .get(&key)
        .cloned()
        .unwrap_or(Reply {
            status: StatusCode::OK,
            after: "0",
        });
    if state.blocked.load(Ordering::SeqCst) {
        state.reached.notify_one();
        state.release.notified().await;
    }
    (
        reply.status,
        [(header::RETRY_AFTER, reply.after)],
        axum::Json(serde_json::json!({"sub":"owner","active":true,"permissions":["cma:*"]})),
    )
}

struct Fixture {
    auth: Arc<AuthState>,
    provider: Arc<Provider>,
    base: String,
    server: tokio::task::JoinHandle<()>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl Fixture {
    async fn new() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let provider = Arc::new(Provider {
            replies: Mutex::new(HashMap::new()),
            calls: Mutex::new(HashMap::new()),
            total: AtomicUsize::new(0),
            blocked: AtomicBool::new(false),
            reached: Notify::new(),
            release: Notify::new(),
        });
        let app = Router::new().fallback(respond).with_state(provider.clone());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let auth = AuthState::in_memory(
            AuthMode::Provider {
                base_url: base.clone(),
                client_id: "cooldown-client".into(),
                client_secret: "fixture-secret".into(),
                public_url: "https://cma.example.test".into(),
            },
            None,
        );
        Self {
            auth,
            provider,
            base,
            server,
        }
    }

    fn reply(&self, path: &str, token: &str, status: StatusCode, after: &'static str) {
        self.provider.replies.lock().unwrap().insert(
            (path.into(), format!("Bearer {token}")),
            Reply { status, after },
        );
    }

    fn calls(&self, path: &str, token: &str) -> usize {
        *self
            .provider
            .calls
            .lock()
            .unwrap()
            .get(&(path.into(), format!("Bearer {token}")))
            .unwrap_or(&0)
    }

    async fn call(
        &self,
        token: &str,
        path: &str,
        endpoint: Endpoint,
    ) -> Result<serde_json::Value, Error> {
        call(&self.auth, token, &format!("{}{path}", self.base), endpoint).await
    }
}

async fn call(
    auth: &AuthState,
    token: &str,
    url: &str,
    endpoint: Endpoint,
) -> Result<serde_json::Value, Error> {
    json(
        auth,
        token,
        url,
        endpoint,
        auth.http.get(url).bearer_auth(token),
        Instant::now() + Duration::from_secs(2),
    )
    .await
}

fn limited(result: Result<serde_json::Value, Error>) -> RetryAfter {
    match result {
        Err(Error::RateLimited(hint)) => hint,
        other => panic!("expected typed throttle, got {other:?}"),
    }
}

#[tokio::test]
async fn concurrent_and_sequential_bursts_dispatch_once_per_scope_during_cooldown() {
    let fixture = Fixture::new().await;
    fixture.reply("/userinfo", "alice", StatusCode::TOO_MANY_REQUESTS, "60");
    fixture.provider.blocked.store(true, Ordering::SeqCst);
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..24 {
        let auth = fixture.auth.clone();
        let url = format!("{}/userinfo", fixture.base);
        tasks.spawn(async move { call(&auth, "alice", &url, Endpoint::Userinfo).await });
    }
    fixture.provider.reached.notified().await;
    fixture.provider.blocked.store(false, Ordering::SeqCst);
    fixture.provider.release.notify_one();
    while let Some(result) = tasks.join_next().await {
        assert!((1..=60).contains(&limited(result.unwrap()).seconds()));
    }
    for _ in 0..24 {
        assert!((1..=60).contains(
            &limited(fixture.call("alice", "/userinfo", Endpoint::Userinfo).await).seconds()
        ));
    }
    assert_eq!(fixture.calls("/userinfo", "alice"), 1);
}

#[tokio::test]
async fn credential_endpoint_url_and_endpoint_kind_do_not_share_cooldown() {
    let fixture = Fixture::new().await;
    fixture.reply("/userinfo", "alice", StatusCode::TOO_MANY_REQUESTS, "60");
    limited(fixture.call("alice", "/userinfo", Endpoint::Userinfo).await);
    assert!(fixture
        .call("bob", "/userinfo", Endpoint::Userinfo)
        .await
        .is_ok());
    assert!(fixture
        .call("alice", "/introspect", Endpoint::Introspection)
        .await
        .is_ok());
    assert!(fixture
        .call("alice", "/replacement-userinfo", Endpoint::Userinfo)
        .await
        .is_ok());
    // Even if a provider uses one URL for both roles, the scopes remain distinct.
    fixture.reply("/userinfo", "alice", StatusCode::OK, "0");
    assert!(fixture
        .call("alice", "/userinfo", Endpoint::Introspection)
        .await
        .is_ok());
    limited(fixture.call("alice", "/userinfo", Endpoint::Userinfo).await);
    assert_eq!(fixture.calls("/userinfo", "alice"), 2);
    assert_eq!(fixture.calls("/userinfo", "bob"), 1);
}

#[tokio::test]
async fn auth_states_do_not_share_negative_state() {
    let fixture = Fixture::new().await;
    fixture.reply("/userinfo", "alice", StatusCode::TOO_MANY_REQUESTS, "60");
    limited(fixture.call("alice", "/userinfo", Endpoint::Userinfo).await);
    let other = AuthState::in_memory(fixture.auth.mode.clone(), None);
    fixture.reply("/userinfo", "alice", StatusCode::OK, "0");
    assert!(call(
        &other,
        "alice",
        &format!("{}/userinfo", fixture.base),
        Endpoint::Userinfo
    )
    .await
    .is_ok());
    limited(fixture.call("alice", "/userinfo", Endpoint::Userinfo).await);
    assert_eq!(fixture.calls("/userinfo", "alice"), 2);
}

#[tokio::test]
async fn retry_after_is_not_shortened_or_extended_by_cache_hits() {
    let fixture = Fixture::new().await;
    fixture.reply("/userinfo", "alice", StatusCode::TOO_MANY_REQUESTS, "2");
    let original = limited(fixture.call("alice", "/userinfo", Endpoint::Userinfo).await);
    tokio::time::pause();
    tokio::time::advance(Duration::from_millis(1500)).await;
    let cached = limited(fixture.call("alice", "/userinfo", Endpoint::Userinfo).await);
    assert_eq!(cached, original);
    assert_eq!(cached.seconds(), 1);
    assert_eq!(fixture.calls("/userinfo", "alice"), 1);
    tokio::time::advance(Duration::from_millis(501)).await;
    tokio::time::resume();
    fixture.reply("/userinfo", "alice", StatusCode::OK, "0");
    assert!(fixture
        .call("alice", "/userinfo", Endpoint::Userinfo)
        .await
        .is_ok());
    assert_eq!(fixture.calls("/userinfo", "alice"), 2);
}

#[tokio::test]
async fn every_success_is_fresh_and_revocation_is_observed_on_the_next_call() {
    let fixture = Fixture::new().await;
    for _ in 0..3 {
        assert!(fixture
            .call("alice", "/userinfo", Endpoint::Userinfo)
            .await
            .is_ok());
    }
    assert_eq!(fixture.calls("/userinfo", "alice"), 3);
    fixture.reply("/userinfo", "alice", StatusCode::UNAUTHORIZED, "60");
    assert!(matches!(
        fixture.call("alice", "/userinfo", Endpoint::Userinfo).await,
        Err(Error::Rejected(_))
    ));
    fixture.reply("/userinfo", "alice", StatusCode::FORBIDDEN, "60");
    assert!(matches!(
        fixture.call("alice", "/userinfo", Endpoint::Userinfo).await,
        Err(Error::Rejected(_))
    ));
    assert_eq!(fixture.calls("/userinfo", "alice"), 5);
}

#[tokio::test]
async fn cancellation_releases_probe_without_detached_work() {
    let fixture = Fixture::new().await;
    fixture.provider.blocked.store(true, Ordering::SeqCst);
    let auth = fixture.auth.clone();
    let url = format!("{}/userinfo", fixture.base);
    let task = tokio::spawn(async move { call(&auth, "alice", &url, Endpoint::Userinfo).await });
    fixture.provider.reached.notified().await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    fixture.provider.blocked.store(false, Ordering::SeqCst);
    fixture.provider.release.notify_one();
    fixture.reply("/userinfo", "alice", StatusCode::TOO_MANY_REQUESTS, "60");
    limited(fixture.call("alice", "/userinfo", Endpoint::Userinfo).await);
    limited(fixture.call("alice", "/userinfo", Endpoint::Userinfo).await);
    assert_eq!(fixture.calls("/userinfo", "alice"), 2);
}

#[tokio::test]
async fn full_capacity_keeps_existing_backoff_and_never_bypasses_admission() {
    let mut fixture = Fixture::new().await;
    Arc::get_mut(&mut fixture.auth)
        .unwrap()
        .identity_cooldowns
        .capacity = 1;
    fixture.reply("/userinfo", "alice", StatusCode::TOO_MANY_REQUESTS, "60");
    limited(fixture.call("alice", "/userinfo", Endpoint::Userinfo).await);
    assert!(matches!(
        fixture.call("bob", "/userinfo", Endpoint::Userinfo).await,
        Err(Error::Unavailable(_))
    ));
    limited(fixture.call("alice", "/userinfo", Endpoint::Userinfo).await);
    assert_eq!(fixture.provider.total.load(Ordering::SeqCst), 1);
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(61)).await;
    tokio::time::resume();
    assert!(fixture
        .call("bob", "/userinfo", Endpoint::Userinfo)
        .await
        .is_ok());
    assert_eq!(fixture.calls("/userinfo", "bob"), 1);
}

#[tokio::test]
async fn a_burst_after_expiry_still_sends_only_one_throttled_probe() {
    let fixture = Fixture::new().await;
    fixture.reply("/userinfo", "alice", StatusCode::TOO_MANY_REQUESTS, "60");
    limited(fixture.call("alice", "/userinfo", Endpoint::Userinfo).await);
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(61)).await;
    tokio::time::resume();
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..16 {
        let auth = fixture.auth.clone();
        let url = format!("{}/userinfo", fixture.base);
        tasks.spawn(async move { call(&auth, "alice", &url, Endpoint::Userinfo).await });
    }
    while let Some(result) = tasks.join_next().await {
        limited(result.unwrap());
    }
    assert_eq!(fixture.calls("/userinfo", "alice"), 2);
}

#[tokio::test]
async fn successful_requests_do_not_fill_negative_cache_capacity() {
    let mut fixture = Fixture::new().await;
    Arc::get_mut(&mut fixture.auth)
        .unwrap()
        .identity_cooldowns
        .capacity = 1;
    for token in ["alice", "bob", "carol", "alice"] {
        assert!(fixture
            .call(token, "/userinfo", Endpoint::Userinfo)
            .await
            .is_ok());
    }
    assert_eq!(fixture.provider.total.load(Ordering::SeqCst), 4);
}
