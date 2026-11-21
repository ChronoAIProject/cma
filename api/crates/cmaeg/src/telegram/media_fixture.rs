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
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
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
type OnSend = Arc<dyn Fn(&Capture) + Send + Sync>;
pub(crate) struct Memory {
    pub echo_controls: AtomicBool,
    pub on_send: Mutex<Option<OnSend>>,
    pub last_media: Mutex<Option<Value>>,
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
            echo_controls: AtomicBool::new(false),
            on_send: Mutex::new(None),
            last_media: Mutex::new(None),
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
    let capture = Capture {
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
    };
    memory.calls.lock().unwrap().push(capture.clone());
    if let Some(callback) = memory.on_send.lock().unwrap().as_ref() {
        callback(&capture);
    }
    let answer = memory
        .answers
        .lock()
        .unwrap()
        .pop_front()
        .unwrap_or_else(|| {
            if memory.echo_controls.load(Ordering::SeqCst) && capture.path.ends_with("/editMessageCaption") {
                let body: Value = serde_json::from_slice(&capture.body).unwrap();
                let mut previous = memory.last_media.lock().unwrap();
                let value = previous.as_mut().unwrap();
                value["caption"] = body["caption"].clone();
                value["reply_markup"] = body["reply_markup"].clone();
                for key in ["caption_entities", "show_caption_above_media"] {
                    if let Some(field) = body.get(key) { value[key] = field.clone(); }
                }
                Answer::json(StatusCode::OK, json!({"ok":true,"result":value}))
            } else if memory.echo_controls.load(Ordering::SeqCst) {
                let markup: Value = serde_json::from_str(&field(&capture, "reply_markup")).unwrap();
                let caption = field(&capture, "caption");
                let media = match capture.path.rsplit('/').next().unwrap() {
                    "sendPhoto"=>"photo", "sendDocument"=>"document", "sendAudio"=>"audio", "sendVideo"=>"video", _=>panic!("expected one media upload"),
                };
                let asset = json!({"file_id":"file_locator","file_unique_id":"stable_file","file_size":4,"width":2,"height":2});
                let asset = if media == "photo" { json!([asset]) } else { asset };
                let value = json!({
                    "message_id":800,"from":{"id":77,"is_bot":true},"chat":{"id":-42},"message_thread_id":7,
                    "reply_markup":markup,"caption":caption,media:asset
                });
                *memory.last_media.lock().unwrap() = Some(value.clone());
                Answer::json(StatusCode::OK, json!({"ok":true,"result":value}))
            } else { Answer::ok() }
        });
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

pub(crate) fn field(capture: &Capture, name: &str) -> String {
    let body = String::from_utf8_lossy(&capture.body);
    let marker = format!("name=\"{name}\"\r\n\r\n");
    body.split(&marker)
        .nth(1)
        .map(|value| value.split("\r\n--").next().unwrap().to_owned())
        .unwrap_or_default()
}
