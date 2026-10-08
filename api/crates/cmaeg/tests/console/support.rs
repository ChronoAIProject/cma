//! A console served on a loopback listener next to readiness and health,
//! wired to a fake NyxID.
use super::fake_nyxid::{console as oauth, FakeNyxid};
use axum::{routing::get, Json, Router};
use cmaeg::{
    console::{clock::Clock, token::TokenSet, Console, ConsoleDeps},
    http::readiness::{readyz, Readiness},
    ConsoleConfig,
};
use cmaeg_core::channel::Secret;
use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tokio::sync::oneshot;

/// A clock tests move by hand.
pub struct ManualClock {
    base: Instant,
    offset_ms: AtomicU64,
}

impl ManualClock {
    pub fn advance(&self, by: Duration) {
        self.offset_ms
            .fetch_add(by.as_millis() as u64, Ordering::SeqCst);
    }
}

impl Clock for ManualClock {
    fn now(&self) -> Instant {
        self.base + Duration::from_millis(self.offset_ms.load(Ordering::SeqCst))
    }
}

/// Render the process metrics, installing the recorder on first use.
pub fn metrics() -> String {
    cmaeg::telemetry::install().unwrap().render()
}

/// The value of one sample, `name{labels}` exactly as rendered.
pub fn sample(rendered: &str, series: &str) -> f64 {
    rendered
        .lines()
        .find_map(|line| line.strip_prefix(&format!("{series} ")))
        .map_or(0.0, |value| value.trim().parse().unwrap())
}

pub struct Harness {
    pub base: String,
    pub nyxid: FakeNyxid,
    pub console: Console,
    pub clock: Arc<ManualClock>,
    pub http: reqwest::Client,
    shutdown: Option<oneshot::Sender<()>>,
}

/// A complete client configuration with the console switched on.
pub fn config() -> ConsoleConfig {
    ConsoleConfig {
        enabled: true,
        client_id: oauth::CLIENT_ID.into(),
        client_secret: Some(Secret::new(oauth::CLIENT_SECRET)),
        ..ConsoleConfig::default()
    }
}

impl Harness {
    /// Serve a console built from `config`, without discovering yet.
    pub async fn start(config: ConsoleConfig) -> Self {
        // Before the console exists, so every console metric is recorded.
        cmaeg::telemetry::install().unwrap();
        let nyxid = FakeNyxid::start().await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let clock = Arc::new(ManualClock {
            base: Instant::now(),
            offset_ms: AtomicU64::new(0),
        });
        let console = Console::new(ConsoleDeps {
            config,
            public_url: base.clone(),
            nyxid_base_url: nyxid.base_url.clone(),
            clock: clock.clone(),
        })
        .unwrap();
        let router = Router::new()
            .route("/readyz", get(readyz).with_state(Readiness::initialized()))
            .route(
                "/healthz",
                get(|| async { Json(serde_json::json!({"status": "ok"})) }),
            )
            .merge(console.router());
        let (shutdown, signal) = oneshot::channel();
        tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    let _ = signal.await;
                })
                .await
                .unwrap();
        });
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        Self {
            base,
            nyxid,
            console,
            clock,
            http,
            shutdown: Some(shutdown),
        }
    }

    /// Serve an enabled console: complete client and a successful discovery.
    pub async fn enabled() -> Self {
        let harness = Self::start(config()).await;
        harness.console.discover_once().await.unwrap();
        assert!(harness.console.is_enabled());
        harness
    }

    /// Tokens the fake NyxID issued for `subject`, as a sign-in would hold.
    pub fn tokens(&self, subject: &str) -> TokenSet {
        let (access, refresh) = self.nyxid.oauth.issue_tokens(subject);
        TokenSet {
            access: Secret::new(access),
            refresh: Secret::new(refresh),
            expires_in: Duration::from_secs(900),
        }
    }

    pub async fn get(&self, path: &str) -> reqwest::Response {
        self.http
            .get(format!("{}{path}", self.base))
            .send()
            .await
            .unwrap()
    }

    pub async fn close(mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        self.nyxid.close().await;
    }
}
