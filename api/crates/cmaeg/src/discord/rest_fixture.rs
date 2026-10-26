use super::DiscordRest;
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
};

#[derive(Clone)]
pub struct Capture {
    pub path: String,
    pub method: String,
    pub authorization: Option<String>,
    pub content_type: Option<String>,
    pub body: Vec<u8>,
}
type OnCall = Arc<dyn Fn(&Capture) + Send + Sync>;
pub struct Memory {
    pub on_call: Mutex<Option<OnCall>>,
    pub calls: Mutex<Vec<Capture>>,
    pub identities: Mutex<Vec<Capture>>,
    pub status: Mutex<StatusCode>,
    pub response: Mutex<Vec<u8>>,
    pub broken: Mutex<bool>,
    pub bot: Mutex<Value>,
    pub responses: Mutex<VecDeque<(StatusCode, Value)>>,
}
pub struct Fixture {
    pub api: DiscordRest,
    pub memory: Arc<Memory>,
    pub base: String,
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
            on_call: Mutex::new(None),
            calls: Mutex::new(Vec::new()),
            identities: Mutex::new(Vec::new()),
            status: Mutex::new(StatusCode::OK),
            response: Mutex::new(
                serde_json::to_vec(&json!({"id":"800","channel_id":"99","author":{"id":"77"}}))
                    .unwrap(),
            ),
            broken: Mutex::new(false),
            bot: Mutex::new(json!({"id":"77","bot":true})),
            responses: Mutex::default(),
        });
        let app = Router::new().fallback(endpoint).with_state(memory.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            api: DiscordRest::fixture(&format!("{base}/api/v10/")),
            memory,
            base,
            task,
        }
    }
    pub fn respond(&self, status: StatusCode, body: Value) {
        *self.memory.status.lock().unwrap() = status;
        *self.memory.response.lock().unwrap() = serde_json::to_vec(&body).unwrap();
    }
}
async fn endpoint(State(state): State<Arc<Memory>>, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    let body = to_bytes(body, 10 * 1024 * 1024).await.unwrap();
    let capture = Capture {
        path: parts.uri.path().into(),
        method: parts.method.to_string(),
        authorization: parts
            .headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned),
        content_type: parts
            .headers
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned),
        body: body.to_vec(),
    };
    if parts.uri.path() == "/api/v10/users/@me" {
        state.identities.lock().unwrap().push(capture);
        return Response::builder()
            .status(StatusCode::OK)
            .body(Body::from(
                serde_json::to_vec(&*state.bot.lock().unwrap()).unwrap(),
            ))
            .unwrap();
    }
    if let Some(on_call) = state.on_call.lock().unwrap().as_ref() {
        on_call(&capture);
    }
    state.calls.lock().unwrap().push(capture);
    if let Some((status, body)) = state.responses.lock().unwrap().pop_front() {
        return Response::builder()
            .status(status)
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
    }
    let body = if *state.broken.lock().unwrap() {
        Body::from_stream(futures_util::stream::iter([Err::<bytes::Bytes, _>(
            std::io::Error::other("fixture lost response"),
        )]))
    } else {
        Body::from(state.response.lock().unwrap().clone())
    };
    Response::builder()
        .status(*state.status.lock().unwrap())
        .body(body)
        .unwrap()
}
