//! Raw and checked Bot API calls against a local fake Telegram server and an
//! in-process transport.
use super::{Error, TelegramClient, TelegramTransport};
use crate::net::exchange::{BodyError, HttpReply, TransportError};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::post,
    Json, Router,
};
use cmaeg_core::channel::Secret;
use futures_util::future::BoxFuture;
use reqwest::{header::HeaderMap, Url};
use serde_json::{json, Value};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

struct Fake {
    client: TelegramClient,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Fake {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn reply(Path((_token, method)): Path<(String, String)>) -> (StatusCode, Json<Value>) {
    match method.as_str() {
        "getMe" => (StatusCode::OK, Json(json!({"ok":true,"result":{"id":77}}))),
        "getFile" => (
            StatusCode::BAD_REQUEST,
            Json(json!({"ok":false,"error_code":400,"description":"Bad Request: file is too big"})),
        ),
        "busy" => (
            StatusCode::TOO_MANY_REQUESTS,
            Json(
                json!({"ok":false,"error_code":429,"description":"Too Many Requests","parameters":{"retry_after":7}}),
            ),
        ),
        "oversized" => (
            StatusCode::OK,
            Json(json!({"ok":true,"result":"x".repeat(40_000)})),
        ),
        _ => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"ok":false,"error_code":500,"description":"Internal Server Error"})),
        ),
    }
}
async fn start() -> Fake {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let app = Router::new().route("/{token}/{method}", post(reply));
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    Fake {
        client: TelegramClient::new(&base, Duration::from_secs(1)).unwrap(),
        task,
    }
}
fn token() -> Secret {
    Secret::new("77:fixture")
}

#[tokio::test]
async fn raw_call_exposes_a_refusal_description_that_the_checked_call_rejects() {
    let fake = start().await;
    let body = json!({"file_id":"file-1"});
    let (status, value) = fake
        .client
        .call_raw(&token(), "getFile", &body)
        .await
        .unwrap();
    assert_eq!(status, reqwest::StatusCode::BAD_REQUEST);
    assert_eq!(value["description"], "Bad Request: file is too big");
    assert_eq!(
        fake.client.call(&token(), "getFile", &body).await,
        Err(Error::Rejected)
    );
}

#[tokio::test]
async fn checked_call_keeps_success_rate_limit_and_server_error_mapping() {
    let fake = start().await;
    let body = json!({});
    assert_eq!(
        fake.client.call(&token(), "getMe", &body).await,
        Ok(json!({"id":77}))
    );
    let (status, value) = fake.client.call_raw(&token(), "busy", &body).await.unwrap();
    assert_eq!(status, reqwest::StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(value["parameters"]["retry_after"], 7);
    assert_eq!(
        fake.client.call(&token(), "busy", &body).await,
        Err(Error::RateLimited(7))
    );
    assert_eq!(
        fake.client.call(&token(), "unexpected", &body).await,
        Err(Error::Unconfirmed)
    );
}

#[tokio::test]
async fn raw_call_refuses_unsafe_tokens_and_oversized_or_unreachable_responses() {
    let fake = start().await;
    let body = json!({});
    assert_eq!(
        fake.client
            .call_raw(&Secret::new("77:invalid/path"), "getMe", &body)
            .await,
        Err(Error::Rejected)
    );
    assert_eq!(
        fake.client.call_raw(&token(), "oversized", &body).await,
        Err(Error::Unconfirmed)
    );
    let closed = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let unreachable = TelegramClient::new(
        &format!("http://{}", closed.local_addr().unwrap()),
        Duration::from_secs(1),
    )
    .unwrap();
    drop(closed);
    assert_eq!(
        unreachable.call_raw(&token(), "getMe", &body).await,
        Err(Error::Unconfirmed)
    );
}

/// A request handed to the transport: URL, headers, body and timeout.
type Request = (Url, HeaderMap, Vec<u8>, Duration);

/// Records each Bot API request the client hands its transport and answers
/// after `delay`, honouring the request's timeout as the real client does.
struct Recording {
    sent: Mutex<Vec<Request>>,
    delay: Duration,
    body: Result<Vec<u8>, BodyError>,
}
impl Recording {
    fn answering(body: Result<Vec<u8>, BodyError>, delay: Duration) -> Arc<Self> {
        Arc::new(Self {
            sent: Mutex::default(),
            delay,
            body,
        })
    }
}
impl TelegramTransport for Recording {
    fn post(
        &self,
        url: Url,
        headers: HeaderMap,
        body: Vec<u8>,
        timeout: Duration,
    ) -> BoxFuture<'_, Result<HttpReply, TransportError>> {
        self.sent
            .lock()
            .unwrap()
            .push((url, headers, body, timeout));
        Box::pin(async move {
            tokio::time::timeout(timeout, tokio::time::sleep(self.delay))
                .await
                .map_err(|_| TransportError::Unavailable)?;
            Ok(HttpReply {
                status: StatusCode::OK,
                headers: HeaderMap::new(),
                body: self.body.clone(),
            })
        })
    }
}

type Captured = Arc<Mutex<Vec<(String, Option<String>, Vec<u8>)>>>;
async fn capture(
    State(captured): State<Captured>,
    uri: axum::http::Uri,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Json<Value> {
    let content_type = headers
        .get("content-type")
        .map(|value| value.to_str().unwrap().to_owned());
    captured
        .lock()
        .unwrap()
        .push((uri.path().to_owned(), content_type, body.to_vec()));
    Json(json!({"ok":true,"result":{"id":77}}))
}

#[tokio::test]
async fn a_fake_transport_receives_what_the_loopback_api_receives() {
    let captured = Captured::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let app = Router::new()
        .route("/{token}/{method}", post(capture))
        .with_state(captured.clone());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let body = json!({"chat_id":"42","text":"hello","reply_markup":{"inline_keyboard":[]}});
    let live = TelegramClient::new(&base, Duration::from_secs(1)).unwrap();
    let live_answer = live.call_raw(&token(), "sendMessage", &body).await.unwrap();
    server.abort();

    let recording = Recording::answering(
        Ok(br#"{"ok":true,"result":{"id":77}}"#.to_vec()),
        Duration::ZERO,
    );
    let faked = TelegramClient::new(&base, Duration::from_secs(1))
        .unwrap()
        .with_transport(recording.clone());
    assert_eq!(
        faked
            .call_raw(&token(), "sendMessage", &body)
            .await
            .unwrap(),
        live_answer
    );
    let (path, content_type, bytes) = captured.lock().unwrap()[0].clone();
    let (url, headers, sent, timeout) = recording.sent.lock().unwrap()[0].clone();
    assert_eq!(url.as_str(), format!("{base}{path}"));
    assert_eq!(path, "/bot77:fixture/sendMessage");
    assert_eq!(
        headers
            .get("content-type")
            .map(|value| value.to_str().unwrap().to_owned()),
        content_type
    );
    assert_eq!(headers.len(), 1);
    assert_eq!(sent, bytes);
    assert_eq!(timeout, Duration::from_secs(1));
}

#[tokio::test(start_paused = true)]
async fn a_fake_sleeping_past_the_timeout_is_unconfirmed() {
    let recording = Recording::answering(Ok(b"{}".to_vec()), Duration::from_secs(60));
    let client = TelegramClient::new("https://api.telegram.org", Duration::from_millis(800))
        .unwrap()
        .with_transport(recording);
    let started = tokio::time::Instant::now();
    assert_eq!(
        client.call_raw(&token(), "getMe", &json!({})).await,
        Err(Error::Unconfirmed)
    );
    assert_eq!(started.elapsed(), Duration::from_millis(800));
}

#[tokio::test(start_paused = true)]
async fn unreadable_answers_are_unconfirmed_and_unsafe_tokens_never_reach_the_transport() {
    for body in [
        Err(BodyError::Oversized),
        Err(BodyError::Lost),
        Ok(b"x".to_vec()),
    ] {
        let client = TelegramClient::new("https://api.telegram.org", Duration::from_secs(1))
            .unwrap()
            .with_transport(Recording::answering(body, Duration::ZERO));
        assert_eq!(
            client.call_raw(&token(), "getMe", &json!({})).await,
            Err(Error::Unconfirmed)
        );
    }
    let recording = Recording::answering(Ok(b"{}".to_vec()), Duration::ZERO);
    let client = TelegramClient::new("https://api.telegram.org", Duration::from_secs(1))
        .unwrap()
        .with_transport(recording.clone());
    assert_eq!(
        client
            .call_raw(&Secret::new("77:invalid/path"), "getMe", &json!({}))
            .await,
        Err(Error::Rejected)
    );
    assert!(recording.sent.lock().unwrap().is_empty());
}
