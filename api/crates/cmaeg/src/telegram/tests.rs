//! Raw and checked Bot API calls against a local fake Telegram server.
use super::{Error, TelegramClient};
use axum::{extract::Path, http::StatusCode, routing::post, Json, Router};
use cmaeg_core::channel::Secret;
use serde_json::{json, Value};
use std::time::Duration;

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
