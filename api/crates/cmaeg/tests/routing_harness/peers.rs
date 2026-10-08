//! Fixture-only HTTP endpoints and the webhook client's replaceable IO boundary.
use super::signer::Signer;
use axum::{
    extract::{Path, State},
    routing::{get, post},
    Json, Router,
};
use cmaeg::delivery::webhook_client::{TransportError, WebhookTransport};
use futures_util::future::BoxFuture;
use reqwest::{header::HeaderMap, Url};
use serde_json::{json, Value};
use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
};
use tokio::task::JoinHandle;

#[derive(Clone, Default)]
pub struct Calls(pub Arc<Mutex<Vec<(String, Value)>>>);
pub struct Peers {
    pub base: String,
    pub calls: Calls,
    task: JoinHandle<()>,
}
impl Drop for Peers {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Peers {
    pub async fn start() -> Self {
        let calls = Calls::default();
        let router = Router::new()
            .route("/jwks", get(|| async { Json(Signer::shared().jwks()) }))
            .route("/api/v1/channel-relay/reply", post(relay))
            .route("/api/v1/channel-relay/send", post(relay))
            .route("/webhook", post(hook))
            .route("/{token}/{method}", post(telegram))
            .with_state(calls.clone());
        let (base, task) = serve(router).await;
        Self { base, calls, task }
    }
    pub fn chat_calls(&self) -> Vec<(String, Value)> {
        self.calls
            .0
            .lock()
            .unwrap()
            .iter()
            .filter(|(kind, _)| kind != "webhook")
            .cloned()
            .collect()
    }
}
async fn relay(State(calls): State<Calls>, Json(body): Json<Value>) -> Json<Value> {
    calls.0.lock().unwrap().push(("relay".into(), body));
    Json(json!({"message_id":"outbound-1","platform_message_id":"42"}))
}
async fn hook(State(calls): State<Calls>, Json(body): Json<Value>) -> Json<Value> {
    calls.0.lock().unwrap().push(("webhook".into(), body));
    Json(json!({"ok":true}))
}
async fn telegram(
    State(calls): State<Calls>,
    Path((_, method)): Path<(String, String)>,
    Json(body): Json<Value>,
) -> Json<Value> {
    let chat = body["chat_id"]
        .as_i64()
        .or_else(|| body["chat_id"].as_str().and_then(|v| v.parse().ok()))
        .unwrap_or(42);
    calls.0.lock().unwrap().push((method.clone(), body));
    Json(
        json!({"ok":true,"result":if method == "sendMessage" || method == "editMessageText" {
        json!({"message_id":42,"chat":{"id":chat}})
    } else { json!(true) }}),
    )
}
pub async fn serve(router: Router) -> (String, JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    (
        base,
        tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        }),
    )
}

/// Preserve production destination validation while relocating this one fixture
/// destination to local HTTP. No other URL can escape the test boundary.
pub struct LocalWebhook(pub String);
impl WebhookTransport for LocalWebhook {
    fn resolve(&self, url: Url) -> BoxFuture<'_, Result<Vec<SocketAddr>, TransportError>> {
        Box::pin(async move {
            assert_eq!(url.as_str(), "https://receiver.example/events");
            Ok(vec!["93.184.216.34:443".parse().unwrap()])
        })
    }
    fn post(
        &self,
        url: Url,
        _: Vec<SocketAddr>,
        headers: HeaderMap,
        body: Arc<Vec<u8>>,
    ) -> BoxFuture<'_, Result<u16, TransportError>> {
        Box::pin(async move {
            assert_eq!(url.as_str(), "https://receiver.example/events");
            reqwest::Client::new()
                .post(format!("{}/webhook", self.0))
                .headers(headers)
                .body(body.as_ref().clone())
                .send()
                .await
                .map(|response| response.status().as_u16())
                .map_err(|_| TransportError::Unavailable)
        })
    }
}
