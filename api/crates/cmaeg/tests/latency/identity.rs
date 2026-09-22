use super::signer::Signer;
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value};
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Instant,
};

pub struct Identity {
    pub base: String,
    pub jwks_calls: Arc<AtomicUsize>,
    pub replies: Arc<Mutex<Vec<(String, Instant)>>>,
    task: tokio::task::JoinHandle<()>,
}
#[derive(Clone)]
struct Data {
    jwks_calls: Arc<AtomicUsize>,
    replies: Arc<Mutex<Vec<(String, Instant)>>>,
}
impl Drop for Identity {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Identity {
    pub async fn start() -> Self {
        let data = Data {
            jwks_calls: Arc::default(),
            replies: Arc::default(),
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let router = Router::new()
            .route(
                "/api/v1/users/me",
                get(|headers: HeaderMap| async move {
                    if headers
                        .get("authorization")
                        .is_some_and(|value| value == "Bearer token-alice")
                    {
                        Json(json!({"subject":"alice"})).into_response()
                    } else {
                        StatusCode::UNAUTHORIZED.into_response()
                    }
                }),
            )
            .route(
                "/jwks",
                get(|State(data): State<Data>| async move {
                    data.jwks_calls.fetch_add(1, Ordering::SeqCst);
                    Json(Signer::shared().jwks())
                }),
            )
            .route("/api/v1/channel-relay/reply", post(reply))
            .with_state(data.clone());
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Self {
            base,
            jwks_calls: data.jwks_calls,
            replies: data.replies,
            task,
        }
    }
}
async fn reply(State(data): State<Data>, headers: HeaderMap, Json(body): Json<Value>) -> Response {
    assert!(headers.contains_key("authorization"));
    let route = body["message_id"].as_str().unwrap().to_owned();
    let mut replies = data.replies.lock().unwrap();
    replies.push((route, Instant::now()));
    Json(json!({"message_id":format!("reply-{}", replies.len()),"platform_message_id":format!("platform-{}", replies.len())})).into_response()
}
