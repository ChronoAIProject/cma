use std::{
    convert::Infallible,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use axum::{
    body::{Body, Bytes},
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Json, Router,
};
use futures_util::{stream, StreamExt};
use serde_json::{json, Value};
use tokio::{net::TcpListener, sync::oneshot, time::sleep};

#[derive(Debug, Clone)]
pub struct FakeProviderConfig {
    pub response_delay: Duration,
    pub busy_once: bool,
    pub conversation_missing_once: bool,
    pub malformed_frame: bool,
    pub disconnect_after: Option<usize>,
}

impl Default for FakeProviderConfig {
    fn default() -> Self {
        Self {
            response_delay: Duration::ZERO,
            busy_once: false,
            conversation_missing_once: false,
            malformed_frame: false,
            disconnect_after: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceivedRequest {
    pub method: String,
    pub path: String,
    pub authorization: Option<String>,
    pub idempotency_key: Option<String>,
    pub body: String,
}

struct StateData {
    config: FakeProviderConfig,
    requests: Mutex<Vec<ReceivedRequest>>,
    busy_once: AtomicBool,
    conversation_missing_once: AtomicBool,
}

pub struct FakeProvider {
    base_url: String,
    state: Arc<StateData>,
    shutdown: Option<oneshot::Sender<()>>,
}

impl FakeProvider {
    pub async fn start(config: FakeProviderConfig) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let busy_once = config.busy_once;
        let missing_once = config.conversation_missing_once;
        let state = Arc::new(StateData {
            config,
            requests: Mutex::new(Vec::new()),
            busy_once: AtomicBool::new(busy_once),
            conversation_missing_once: AtomicBool::new(missing_once),
        });
        let router = Router::new()
            .route("/.well-known/agent-card.json", get(agent_card))
            .route(
                "/api/v1/cmaeg/bindings/{binding_id}",
                put(bind).delete(unbind),
            )
            .route(
                "/api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}",
                put(ensure_conversation).delete(delete_conversation),
            )
            .route("/api/v2/responses", post(responses))
            .with_state(state.clone());
        let (shutdown, signal) = oneshot::channel();
        tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    let _ = signal.await;
                })
                .await
                .unwrap();
        });
        Self {
            base_url: format!("http://{address}"),
            state,
            shutdown: Some(shutdown),
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn requests(&self) -> Vec<ReceivedRequest> {
        self.state.requests.lock().unwrap().clone()
    }

    pub async fn close(mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
    }
}

async fn agent_card() -> Json<Value> {
    Json(
        json!({"name":"fake","capabilities":{"streaming":true,"activity_items":true,"client_tools":true,"cancel":true,"rich_input":true}}),
    )
}

async fn bind(
    State(state): State<Arc<StateData>>,
    Path(binding_id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    record(
        &state,
        "PUT",
        format!("/api/v1/cmaeg/bindings/{binding_id}"),
        headers,
        body,
    );
    Json(json!({"binding_id":binding_id,"level":"l3","honoured":["profile"],"refused":[]}))
        .into_response()
}

async fn unbind(
    State(state): State<Arc<StateData>>,
    Path(binding_id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    record(
        &state,
        "DELETE",
        format!("/api/v1/cmaeg/bindings/{binding_id}"),
        headers,
        body,
    );
    StatusCode::NO_CONTENT.into_response()
}

async fn ensure_conversation(
    State(state): State<Arc<StateData>>,
    Path((binding_id, conversation_id)): Path<(String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    record(
        &state,
        "PUT",
        format!("/api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}"),
        headers,
        body,
    );
    if state
        .conversation_missing_once
        .swap(false, Ordering::SeqCst)
    {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"code":"conversation_missing"})),
        )
            .into_response();
    }
    StatusCode::CREATED.into_response()
}

async fn delete_conversation(
    State(state): State<Arc<StateData>>,
    Path((binding_id, conversation_id)): Path<(String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    record(
        &state,
        "DELETE",
        format!("/api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}"),
        headers,
        body,
    );
    StatusCode::NO_CONTENT.into_response()
}

async fn responses(
    State(state): State<Arc<StateData>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    record(&state, "POST", "/api/v2/responses".into(), headers, body);
    if state.busy_once.swap(false, Ordering::SeqCst) {
        return (StatusCode::CONFLICT, Json(json!({"code":"session_busy"}))).into_response();
    }
    let mut frames = vec![
        "event: response.created\ndata: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_fake\"}}\n\n".to_owned(),
        "event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"item_id\":\"msg_1\",\"delta\":\"hello\"}\n\n".to_owned(),
    ];
    if state.config.malformed_frame {
        frames.push("data: {malformed}\n\n".to_owned());
    } else {
        frames.push("event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_fake\"}}\n\n".to_owned());
    }
    if let Some(count) = state.config.disconnect_after {
        frames.truncate(count);
    }
    let delay = state.config.response_delay;
    let stream =
        stream::iter(frames.into_iter().enumerate()).then(move |(index, frame)| async move {
            if index == 0 && !delay.is_zero() {
                sleep(delay).await;
            }
            Ok::<Bytes, Infallible>(Bytes::from(frame))
        });
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/event-stream")
        .body(Body::from_stream(stream))
        .unwrap()
}

fn record(state: &StateData, method: &str, path: String, headers: HeaderMap, body: Bytes) {
    state.requests.lock().unwrap().push(ReceivedRequest {
        method: method.into(),
        path,
        authorization: headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned),
        idempotency_key: headers
            .get("idempotency-key")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned),
        body: String::from_utf8_lossy(&body).into_owned(),
    });
}
