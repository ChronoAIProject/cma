//! Local synthetic IdP; observations contain no identities or credentials.
use super::*;
use axum::{
    extract::State,
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value};
use std::{
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::Instant,
};
use tokio::sync::Notify;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct EndpointSnapshot {
    pub(crate) requests: u64,
    pub(crate) completed: u64,
    pub(crate) timed: u64,
    /// Handler work only, excluding transport, serialization and CMA verification.
    pub(crate) elapsed_nanos: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ProviderSnapshot {
    pub(crate) discovery: EndpointSnapshot,
    pub(crate) userinfo: EndpointSnapshot,
    pub(crate) introspection: EndpointSnapshot,
}

#[derive(Default)]
struct Counts {
    requests: AtomicU64,
    completed: AtomicU64,
    timed: AtomicU64,
    elapsed_nanos: AtomicU64,
}
impl Counts {
    fn start(&self, timing: bool) -> Option<Instant> {
        self.requests.fetch_add(1, Ordering::SeqCst);
        timing.then(Instant::now)
    }
    fn finish(&self, start: Option<Instant>) {
        if let Some(start) = start {
            self.elapsed_nanos.fetch_add(
                u64::try_from(start.elapsed().as_nanos()).unwrap(),
                Ordering::SeqCst,
            );
            self.timed.fetch_add(1, Ordering::SeqCst);
        }
        self.completed.fetch_add(1, Ordering::SeqCst);
    }
    fn snapshot(&self) -> EndpointSnapshot {
        EndpointSnapshot {
            requests: self.requests.load(Ordering::SeqCst),
            completed: self.completed.load(Ordering::SeqCst),
            timed: self.timed.load(Ordering::SeqCst),
            elapsed_nanos: self.elapsed_nanos.load(Ordering::SeqCst),
        }
    }
}

pub(super) struct Provider {
    subject: SubjectId,
    metadata: Value,
    pub(super) denied: AtomicBool,
    pub(super) block: AtomicBool,
    pub(super) entered: Notify,
    pub(super) release: Notify,
    pub(super) timing: AtomicBool,
    discovery: Counts,
    userinfo: Counts,
    introspection: Counts,
}
impl Provider {
    pub(super) fn new(subject: SubjectId, origin: &str) -> Arc<Self> {
        Arc::new(Self {
            subject,
            metadata: json!({
                "authorization_endpoint":format!("{origin}/authorize"),
                "token_endpoint":format!("{origin}/token"),
                "userinfo_endpoint":format!("{origin}/userinfo"),
                "introspection_endpoint":format!("{origin}/introspect"),
            }),
            denied: AtomicBool::new(false),
            block: AtomicBool::new(false),
            entered: Notify::new(),
            release: Notify::new(),
            timing: AtomicBool::new(false),
            discovery: Counts::default(),
            userinfo: Counts::default(),
            introspection: Counts::default(),
        })
    }

    pub(super) fn router(self: &Arc<Self>) -> Router {
        Router::new()
            .route("/.well-known/openid-configuration", get(discovery))
            .route("/userinfo", get(userinfo))
            .route("/introspect", post(introspect))
            .with_state(self.clone())
    }

    pub(super) fn snapshot(&self) -> ProviderSnapshot {
        ProviderSnapshot {
            discovery: self.discovery.snapshot(),
            userinfo: self.userinfo.snapshot(),
            introspection: self.introspection.snapshot(),
        }
    }
}

async fn discovery(State(provider): State<Arc<Provider>>) -> Json<Value> {
    let start = provider
        .discovery
        .start(provider.timing.load(Ordering::SeqCst));
    let result = Json(provider.metadata.clone());
    provider.discovery.finish(start);
    result
}

async fn userinfo(State(provider): State<Arc<Provider>>) -> Json<Value> {
    let start = provider
        .userinfo
        .start(provider.timing.load(Ordering::SeqCst));
    if provider.block.load(Ordering::SeqCst) {
        provider.entered.notify_one();
        provider.release.notified().await;
    }
    let result = Json(json!({"sub":provider.subject.as_str()}));
    provider.userinfo.finish(start);
    result
}

async fn introspect(State(provider): State<Arc<Provider>>) -> Json<Value> {
    let start = provider
        .introspection
        .start(provider.timing.load(Ordering::SeqCst));
    let result = Json(json!({
        "active":!provider.denied.load(Ordering::SeqCst),
        "sub":provider.subject.as_str(),
        "permissions":["cma:sandbox:use"],
    }));
    provider.introspection.finish(start);
    result
}
