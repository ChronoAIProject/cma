//! The gateway console at `/console`: people sign in with their NyxID account
//! and see the channels they own.
//!
//! The console is optional and fails soft. With `CMAEG_CONSOLE_ENABLED` off
//! no route exists and nothing is allocated. On, it never blocks startup or
//! readiness: until its OAuth client is configured and NyxID's discovery
//! document has passed validation, every `/console*` request gets a generic
//! 503 page while ingress keeps serving.
mod oidc;
pub(crate) mod pages;

use std::{
    sync::{Arc, RwLock},
    time::Duration,
};

use axum::{
    extract::{Request, State},
    middleware::{from_fn_with_state, Next},
    response::Response,
    routing::any,
    Router,
};
use reqwest::Client;

pub use oidc::{DiscoveryError, OidcEndpoints};

use oidc::DiscoverySink as _;

use crate::{telemetry::names, ConsoleConfig};

/// What the console is built from.
pub struct ConsoleDeps {
    pub config: ConsoleConfig,
    /// `CMAEG_PUBLIC_URL`: the only origin console forms may come from and
    /// redirects may point to.
    pub public_url: String,
    /// `CMAEG_NYXID_BASE_URL`: the only origin console endpoints may use.
    pub nyxid_base_url: String,
}

/// The console's shared state. Built only while `CMAEG_CONSOLE_ENABLED` is
/// on; cloning shares it.
#[derive(Clone)]
pub struct Console {
    inner: Arc<Inner>,
}

struct Inner {
    nyxid_base_url: String,
    http: Client,
    /// The client setting whose absence keeps the console disabled.
    missing_key: Option<&'static str>,
    endpoints: RwLock<Option<Arc<OidcEndpoints>>>,
}

impl Console {
    pub fn new(deps: ConsoleDeps) -> Result<Self, reqwest::Error> {
        let http = Client::builder()
            .connect_timeout(Duration::from_secs(2))
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(format!("cmaeg/{}", env!("CARGO_PKG_VERSION")))
            .build()?;
        let missing_key = deps.config.missing_client_key();
        if let Some(key) = missing_key {
            tracing::error!(
                key,
                "console client is not configured; /console answers 503 until it is"
            );
        }
        crate::telemetry::describe_console();
        metrics::gauge!(names::CONSOLE_ENABLED).set(0);
        Ok(Self {
            inner: Arc::new(Inner {
                nyxid_base_url: deps.nyxid_base_url,
                http,
                missing_key,
                endpoints: RwLock::new(None),
            }),
        })
    }

    /// The `/console*` routes, every one behind the enablement gate.
    pub fn router(&self) -> Router {
        Router::new()
            .route("/console", any(not_found))
            .route("/console/{*rest}", any(not_found))
            .layer(from_fn_with_state(self.clone(), gate))
    }

    /// True once the client is configured and discovery has succeeded.
    pub fn is_enabled(&self) -> bool {
        self.inner.missing_key.is_none() && self.endpoints().is_some()
    }

    pub(crate) fn endpoints(&self) -> Option<Arc<OidcEndpoints>> {
        self.inner
            .endpoints
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// One discovery attempt, publishing its result as the background task
    /// does. Tests and the task share it.
    pub async fn discover_once(&self) -> Result<(), DiscoveryError> {
        let result = oidc::fetch(&self.inner.http, &self.inner.nyxid_base_url).await;
        match &result {
            Ok(endpoints) => self.inner.publish(Some(Arc::new(endpoints.clone()))),
            Err(DiscoveryError::Mismatch(_)) => self.inner.publish(None),
            Err(DiscoveryError::Unavailable) => {}
        }
        result.map(|_| ())
    }

    /// Start discovery, unless the client is missing: then nothing could be
    /// enabled and NyxID is never called.
    pub fn spawn_discovery(&self) -> Option<tokio::task::JoinHandle<()>> {
        if self.inner.missing_key.is_some() {
            return None;
        }
        let inner = self.inner.clone();
        let sink: Arc<dyn oidc::DiscoverySink> = self.inner.clone();
        Some(tokio::spawn(oidc::run(sink, move || {
            let inner = inner.clone();
            async move { oidc::fetch(&inner.http, &inner.nyxid_base_url).await }
        })))
    }
}

impl oidc::DiscoverySink for Inner {
    fn publish(&self, endpoints: Option<Arc<OidcEndpoints>>) {
        let enabled = self.missing_key.is_none() && endpoints.is_some();
        *self
            .endpoints
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = endpoints;
        metrics::gauge!(names::CONSOLE_ENABLED).set(if enabled { 1.0 } else { 0.0 });
    }

    fn current(&self) -> bool {
        self.endpoints
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_some()
    }
}

async fn not_found() -> Response {
    pages::not_found()
}

/// Answer every console request with the generic 503 page until the console
/// is enabled.
async fn gate(State(console): State<Console>, request: Request, next: Next) -> Response {
    if !console.is_enabled() {
        return pages::unavailable();
    }
    next.run(request).await
}

/// Build the console when `CMAEG_CONSOLE_ENABLED` is on and start its
/// background work; `None` while it is off, so nothing is allocated.
pub(crate) fn compose(config: &crate::Config) -> Result<Option<Router>, reqwest::Error> {
    if !config.console.enabled {
        return Ok(None);
    }
    let console = Console::new(ConsoleDeps {
        config: config.console.clone(),
        public_url: config.management.public_url.clone(),
        nyxid_base_url: config.management.nyxid_base_url.clone(),
    })?;
    // Detached: discovery runs for the life of the process.
    drop(console.spawn_discovery());
    Ok(Some(console.router()))
}
