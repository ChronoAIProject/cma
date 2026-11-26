use std::{
    net::SocketAddr,
    num::NonZeroU32,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
};

use axum::{
    body::{to_bytes, Body},
    extract::ConnectInfo,
    http::{header, Request, StatusCode},
    middleware,
    response::Response,
    routing::{get, post},
    Json, Router,
};
use layer2_core::{
    ActivityQuery, ActivityRecord, ActivityService, ActivityStore, DirectoryQuery, Page,
    PublicIdentity, RegisteredUser, StoreError, SubjectId, UserDirectory, UserDirectoryStore,
};
use mongodb::{Client, Database};
use serde_json::{json, Value};
use tower::ServiceExt as _;

use crate::{
    auth::{persistence::MemoryAuthStore, AuthMode, AuthState, ProviderMetadata, SignInRecords},
    config::{RateLimitConfig, TrustedProxyConfig},
    rate_limit::{self, RateLimiters},
    store::request_limits::MongoRequestLimits,
};

pub(super) const SUBJECT: &str = "synthetic-signup-owner";
pub(super) const CLOCK: i64 = 1_800_000_000_000;

#[derive(Default)]
pub(super) struct Observations {
    pub users: Mutex<Vec<PublicIdentity>>,
    pub activities: Mutex<Vec<ActivityRecord>>,
    pub exchanges: AtomicUsize,
}

#[async_trait::async_trait]
impl UserDirectoryStore for Observations {
    async fn observe(&self, identity: &PublicIdentity) -> Result<(), StoreError> {
        self.users.lock().unwrap().push(identity.clone());
        Ok(())
    }

    async fn user(&self, _: &SubjectId) -> Result<Option<RegisteredUser>, StoreError> {
        panic!("this qualification does not enable registration capacity")
    }

    async fn list(&self, _: &DirectoryQuery) -> Result<Page<RegisteredUser>, StoreError> {
        panic!("sign-in must not list the directory")
    }
}

#[async_trait::async_trait]
impl ActivityStore for Observations {
    async fn append(&self, record: &ActivityRecord) -> Result<(), StoreError> {
        self.activities.lock().unwrap().push(record.clone());
        Ok(())
    }

    async fn list(&self, _: &ActivityQuery) -> Result<Page<ActivityRecord>, StoreError> {
        panic!("sign-in must not list activity")
    }
}

pub(super) struct Fixture {
    pub database: Database,
    pub auth: Arc<AuthState>,
    pub store: Arc<MemoryAuthStore>,
    pub observed: Arc<Observations>,
    provider: tokio::task::JoinHandle<()>,
}

impl Fixture {
    pub async fn new() -> Self {
        let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
            .expect("disposable Mongo replica set required");
        let database = Client::with_uri_str(uri).await.unwrap().database(&format!(
            "cma_signup_abuse_{}",
            uuid::Uuid::new_v4().simple()
        ));
        let observed = Arc::new(Observations::default());
        let exchanges = observed.clone();
        let token = uuid::Uuid::new_v4().to_string();
        let provider = Router::new()
            .route(
                "/token",
                post(move || {
                    exchanges.exchanges.fetch_add(1, Ordering::SeqCst);
                    let token = token.clone();
                    async move {
                        Json(json!({
                            "access_token": token,
                            "refresh_token": uuid::Uuid::new_v4().to_string(),
                            "expires_in": 3600,
                        }))
                    }
                }),
            )
            .route(
                "/userinfo",
                get(|| async {
                    Json(json!({
                        "sub": SUBJECT,
                        "permissions": ["cma:sandbox:use", "cma:trigger:launch"],
                    }))
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let provider = tokio::spawn(async move {
            axum::serve(listener, provider).await.unwrap();
        });
        let store = Arc::new(MemoryAuthStore::default());
        let auth = AuthState::with_store(
            AuthMode::Provider {
                base_url: base.clone(),
                public_url: "https://cma.example.test".into(),
                client_id: "synthetic-client".into(),
                client_secret: uuid::Uuid::new_v4().to_string(),
            },
            Some(SignInRecords {
                users: UserDirectory::new(observed.clone()),
                activity: ActivityService::new(observed.clone()),
            }),
            store.clone(),
        );
        *auth.fixture_metadata().write().await = Some(ProviderMetadata {
            authorization_endpoint: format!("{base}/authorize"),
            token_endpoint: format!("{base}/token"),
            userinfo_endpoint: Some(format!("{base}/userinfo")),
            introspection_endpoint: None,
        });
        Self {
            database,
            auth,
            store,
            observed,
            provider,
        }
    }

    pub async fn replica(&self, config: RateLimitConfig, clock: i64) -> Router {
        let mut store = MongoRequestLimits::open(&self.database).await.unwrap();
        store.set_test_clock_millis(clock);
        let limits =
            RateLimiters::from_config(config, TrustedProxyConfig::sample(), Arc::new(store));
        crate::auth::router(self.auth.clone())
            .layer(middleware::from_fn_with_state(
                limits.clone(),
                rate_limit::enforce,
            ))
            .layer(middleware::from_fn_with_state(
                self.auth.clone(),
                crate::auth::attach_subject,
            ))
            .layer(middleware::from_fn_with_state(limits, rate_limit::admit))
    }

    pub fn assert_no_sign_in_effects(&self) {
        assert_eq!(self.observed.exchanges.load(Ordering::SeqCst), 0);
        assert!(self.observed.users.lock().unwrap().is_empty());
        assert!(self.observed.activities.lock().unwrap().is_empty());
    }

    pub async fn close(self) {
        self.database.drop().await.unwrap();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.provider.abort();
    }
}

pub(super) fn config(admission: u32, default: u32) -> RateLimitConfig {
    let n = |value| NonZeroU32::new(value).unwrap();
    RateLimitConfig {
        admission_per_minute: n(admission),
        health_per_minute: n(120),
        default_per_minute: n(default),
        // GET auth routes must not consume this deliberately smaller tier.
        create_per_minute: n(1),
        input_per_minute: n(120),
        control_per_minute: n(120),
        response_input_tier_enabled: false,
        control_tier_enabled: false,
        default_lease_enabled: false,
        anonymous_public: crate::config::AnonymousPublicConfig {
            enabled: false,
            per_minute: n(1200),
            concurrency: n(16),
        },
    }
}

pub(super) async fn call(app: &Router, path: &str, cookie: &str, peer: u8) -> Response {
    let mut request = Request::get(path)
        .header(header::COOKIE, cookie)
        .body(Body::empty())
        .unwrap();
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([192, 0, 2, peer], 443))));
    app.clone().oneshot(request).await.unwrap()
}

pub(super) async fn assert_limited(response: Response) {
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "application/problem+json"
    );
    assert!(
        response.headers()[header::RETRY_AFTER]
            .to_str()
            .unwrap()
            .parse::<u64>()
            .unwrap()
            > 0
    );
    assert!(!response.headers().contains_key(header::SET_COOKIE));
    let problem: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 8192).await.unwrap()).unwrap();
    assert_eq!(problem["status"], 429);
    assert_eq!(problem["code"], "rate_limited");
}
