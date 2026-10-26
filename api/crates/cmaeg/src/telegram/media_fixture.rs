//! Local HTTP boundary for native media tests; never contacts a platform.
use super::TelegramClient;
use axum::{
    body::{to_bytes, Body},
    extract::{Request, State},
    http::StatusCode,
    response::Response,
    Router,
};
use serde_json::{json, Value};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::Duration,
};

#[derive(Clone)]
pub(crate) struct Capture {
    pub path: String,
    pub content_type: Option<String>,
    pub authorization: Option<String>,
    pub body: Vec<u8>,
}
pub(crate) struct Answer {
    pub status: StatusCode,
    pub body: Vec<u8>,
    pub broken: bool,
    pub delay: Duration,
}
impl Answer {
    pub fn json(status: StatusCode, value: Value) -> Self {
        Self {
            status,
            body: serde_json::to_vec(&value).unwrap(),
            broken: false,
            delay: Duration::ZERO,
        }
    }
    pub fn ok() -> Self {
        Self::json(
            StatusCode::OK,
            json!({"ok":true,"result":{"message_id":800,"chat":{"id":-42},"message_thread_id":7}}),
        )
    }
}
pub(crate) struct Memory {
    pub calls: Mutex<Vec<Capture>>,
    pub answers: Mutex<VecDeque<Answer>>,
}
pub(crate) struct Fixture {
    pub api: TelegramClient,
    pub memory: Arc<Memory>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Fixture {
    pub async fn new() -> Self {
        let memory = Arc::new(Memory {
            calls: Mutex::default(),
            answers: Mutex::default(),
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let api = TelegramClient::new(
            &format!("http://{}", listener.local_addr().unwrap()),
            Duration::from_secs(2),
        )
        .unwrap();
        let app = Router::new().fallback(endpoint).with_state(memory.clone());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self { api, memory, task }
    }
    pub fn answer(&self, answer: Answer) {
        self.memory.answers.lock().unwrap().push_back(answer);
    }
    pub fn calls(&self) -> Vec<Capture> {
        self.memory.calls.lock().unwrap().clone()
    }
}
async fn endpoint(State(memory): State<Arc<Memory>>, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    let bytes = to_bytes(body, 9 * 1024 * 1024).await.unwrap();
    memory.calls.lock().unwrap().push(Capture {
        path: parts.uri.path().to_owned(),
        content_type: parts
            .headers
            .get("content-type")
            .map(|h| h.to_str().unwrap().to_owned()),
        authorization: parts
            .headers
            .get("authorization")
            .map(|h| h.to_str().unwrap().to_owned()),
        body: bytes.to_vec(),
    });
    let answer = memory
        .answers
        .lock()
        .unwrap()
        .pop_front()
        .unwrap_or_else(Answer::ok);
    tokio::time::sleep(answer.delay).await;
    let body = if answer.broken {
        Body::from_stream(futures_util::stream::iter([Err::<bytes::Bytes, _>(
            std::io::Error::other("lost fixture response"),
        )]))
    } else {
        Body::from(answer.body)
    };
    Response::builder()
        .status(answer.status)
        .body(body)
        .unwrap()
}
