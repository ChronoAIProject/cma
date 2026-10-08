//! A console served on a loopback listener next to readiness and health,
//! wired to a fake NyxID.
use super::fake_nyxid::{console as oauth, FakeNyxid};
use axum::{routing::get, Json, Router};
use cmaeg::{
    console::{Console, ConsoleDeps},
    http::readiness::{readyz, Readiness},
    ConsoleConfig,
};
use cmaeg_core::channel::Secret;
use tokio::sync::oneshot;

pub struct Harness {
    pub base: String,
    pub nyxid: FakeNyxid,
    pub console: Console,
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
        let nyxid = FakeNyxid::start().await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let console = Console::new(ConsoleDeps {
            config,
            public_url: base.clone(),
            nyxid_base_url: nyxid.base_url.clone(),
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
