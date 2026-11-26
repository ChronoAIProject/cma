#[path = "fake_provider/context.rs"]
mod context;
use context::event_context;
#[path = "fake_provider/reset.rs"]
mod reset;

use std::{
    convert::Infallible,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
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
use tokio::{
    net::TcpListener,
    sync::{oneshot, Notify, Semaphore},
    time::sleep,
};

#[derive(Debug, Clone)]
pub struct FakeProviderConfig {
    pub reset: reset::Controls,
    pub response_refusal: Option<(u16, String)>,
    pub inspection_agent: Option<(String, String)>,
    pub response_delay: Duration,
    pub terminal_delay: Duration,
    pub terminal_gate: Option<Arc<Semaphore>>,
    pub header_delay: Duration,
    /// A header delay for turns whose input carries an attachment block only,
    /// so a test can hold media turns while text turns answer at once.
    pub attachment_delay: Duration,
    /// Holds attachment response headers until the test releases one permit per turn.
    pub attachment_gate: Option<Arc<Semaphore>>,
    pub missing_turn_once: bool,
    pub busy_always: bool,
    pub frames: Option<Vec<Value>>,
    pub busy_once: bool,
    pub busy_retry_after: Option<u64>,
    pub conversation_missing_once: bool,
    /// Ordered conversation PUT refusals, before the default successful creation.
    pub conversation_statuses: Vec<(u16, String)>,
    pub conversation_delete_status: Option<(u16, String)>,
    pub malformed_frame: bool,
    pub disconnect_after: Option<usize>,
    pub recovery_status: Option<&'static str>,
    pub recovery_replay_status: Option<&'static str>,
    pub recovery_wrong_metadata: bool,
    pub reject_binding: bool,
    pub binding_pause: Option<Arc<BindingPause>>,
    /// Exact binding PUT refusals by zero-based attempt; other attempts succeed.
    pub binding_failures: std::collections::BTreeMap<usize, (u16, Value)>,
    pub text: String,
    pub unknown_event: bool,
    pub in_progress_first: bool,
    /// Statuses that binding deletions answer instead of 204, by binding ID.
    pub unbind_status: std::collections::BTreeMap<String, u16>,
    /// Model durable exact command admission rather than replaying every POST.
    pub exact_admission: bool,
    pub admission_barrier: Option<Arc<tokio::sync::Barrier>>,
    pub context_status: Option<u16>,
    pub context_override: Option<Value>,
    pub context_delay: Duration,
    pub context_steps: Vec<(Duration, Option<u16>, Option<String>)>,
}

#[derive(Debug)]
pub struct BindingPause {
    pub accepted: Notify,
    pub release: Semaphore,
}

impl Default for FakeProviderConfig {
    fn default() -> Self {
        Self {
            reset: Default::default(),
            response_refusal: None,
            inspection_agent: None,
            response_delay: Duration::ZERO,
            terminal_delay: Duration::ZERO,
            terminal_gate: None,
            header_delay: Duration::ZERO,
            attachment_delay: Duration::ZERO,
            attachment_gate: None,
            missing_turn_once: false,
            busy_always: false,
            frames: None,
            busy_once: false,
            busy_retry_after: None,
            conversation_missing_once: false,
            conversation_statuses: Vec::new(),
            conversation_delete_status: None,
            malformed_frame: false,
            disconnect_after: None,
            recovery_status: None,
            recovery_replay_status: None,
            recovery_wrong_metadata: false,
            reject_binding: false,
            binding_pause: None,
            binding_failures: std::collections::BTreeMap::new(),
            text: "hello".into(),
            unknown_event: false,
            in_progress_first: false,
            unbind_status: std::collections::BTreeMap::new(),
            exact_admission: false,
            admission_barrier: None,
            context_status: None,
            context_override: None,
            context_delay: Duration::ZERO,
            context_steps: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceivedRequest {
    pub received_at: Instant,
    pub method: String,
    pub path: String,
    pub authorization: Option<String>,
    pub idempotency_key: Option<String>,
    pub content_type: Option<String>,
    pub body: String,
}

struct StateData {
    config: FakeProviderConfig,
    /// Every request in arrival order, event-context reads included.
    requests: Mutex<Vec<ReceivedRequest>>,
    admitted: Mutex<Vec<ReceivedRequest>>,
    busy_once: AtomicBool,
    conversation_missing_once: AtomicBool,
    missing_turn_once: AtomicBool,
    conversation_attempt: AtomicUsize,
    binding_attempt: AtomicUsize,
    context_attempt: AtomicUsize,
    resets: Mutex<reset::Ledger>,
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
        let missing_turn_once = config.missing_turn_once;
        let state = Arc::new(StateData {
            config,
            requests: Mutex::new(Vec::new()),
            admitted: Mutex::new(Vec::new()),
            busy_once: AtomicBool::new(busy_once),
            conversation_missing_once: AtomicBool::new(missing_once),
            missing_turn_once: AtomicBool::new(missing_turn_once),
            conversation_attempt: AtomicUsize::new(0),
            binding_attempt: AtomicUsize::new(0),
            context_attempt: AtomicUsize::new(0),
            resets: Mutex::new(Default::default()),
        });
        let router = Router::new()
            .route("/.well-known/agent-card.json", get(agent_card))
            .route(
                "/api/v1/cmaeg/bindings/{binding_id}",
                put(bind).delete(unbind),
            )
            .route(
                "/api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}",
                put(ensure_conversation).delete(delete_conversation).get(inspect_conversation),
            )
            .route("/api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}/events/{event_id}", get(event_context))
            .route("/api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}/resets", post(reset::reset))
            .route("/api/v2/responses", post(responses))
            .route("/api/v2/responses/{response_id}", get(recovery_snapshot))
            .route("/api/v2/responses/{response_id}/events", get(recovery_events))
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

    /// The requests that change provider state, in arrival order. Event-context
    /// reads are left out, so the call counts tests assert stay independent
    /// of the agent's own context lookups.
    pub fn requests(&self) -> Vec<ReceivedRequest> {
        self.log()
            .into_iter()
            .filter(|request| request.method != "GET")
            .collect()
    }

    /// Every request in arrival order, including event-context reads.
    pub fn log(&self) -> Vec<ReceivedRequest> {
        self.state.requests.lock().unwrap().clone()
    }

    // This fixture is compiled independently by many integration test crates;
    // only admission/replay tests inspect its accepted-command ledger.
    #[allow(dead_code)]
    pub fn admitted(&self) -> Vec<ReceivedRequest> {
        self.state.admitted.lock().unwrap().clone()
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

fn recovery_response(state: &StateData, id: &str, status: &str) -> Option<Value> {
    if id != "resp_fake" || state.config.recovery_status.is_none() {
        return None;
    }
    let requests = state.requests.lock().unwrap();
    let submission = requests
        .iter()
        .find(|request| request.method == "POST" && request.path == "/api/v2/responses")?;
    let request: Value = serde_json::from_str(&submission.body).unwrap();
    let metadata = if state.config.recovery_wrong_metadata {
        json!({"cmaeg.event":"unrelated"})
    } else {
        request["metadata"].clone()
    };
    let mut response = json!({
        "id":id, "object":"response", "status":status,
        "conversation":{"id":"agt_test"}, "metadata":metadata,
        "output":[{"type":"message","content":[{"type":"output_text","text":"recovered answer"}]}]
    });
    if status == "failed" {
        response["error"] =
            json!({"code":"provider_test_failed","message":"provider test failure"});
    }
    if status == "incomplete" {
        response["incomplete_details"] = json!({"reason":"test_incomplete"});
    }
    Some(response)
}

async fn recovery_snapshot(
    State(state): State<Arc<StateData>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    record(
        &state,
        "GET",
        format!("/api/v2/responses/{id}"),
        headers,
        Bytes::new(),
    );
    state
        .config
        .recovery_status
        .and_then(|status| recovery_response(&state, &id, status))
        .map_or_else(
            || StatusCode::NOT_FOUND.into_response(),
            |value| Json(value).into_response(),
        )
}

async fn recovery_events(
    State(state): State<Arc<StateData>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    record(
        &state,
        "GET",
        format!("/api/v2/responses/{id}/events"),
        headers,
        Bytes::new(),
    );
    let status = state.config.recovery_replay_status.unwrap_or("completed");
    let Some(response) = recovery_response(&state, &id, status) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let event = format!("response.{status}");
    let frame = json!({
        "type":event,"sequence_number":0,"response_id":id,"response":response
    });
    (
        [
            ("content-type", "text/event-stream"),
            ("x-cma-response-id", &id),
        ],
        format!("event: {event}\nid: 0\ndata: {frame}\n\n"),
    )
        .into_response()
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
    let attempt = state.binding_attempt.fetch_add(1, Ordering::SeqCst);
    if let Some((status, response)) = state.config.binding_failures.get(&attempt) {
        return (
            StatusCode::from_u16(*status).unwrap(),
            Json(response.clone()),
        )
            .into_response();
    }
    if state.config.reject_binding {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({"code":"profile_refused"})),
        )
            .into_response();
    }
    if let Some(pause) = &state.config.binding_pause {
        pause.accepted.notify_one();
        pause.release.acquire().await.unwrap().forget();
    }
    Json(json!({"binding_id":binding_id,"level":"l3","honoured":["profile"],"refused":[{"field":"context.mcp_servers","reason":"not supported"}]}))
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
    match state.config.unbind_status.get(&binding_id) {
        Some(status) => StatusCode::from_u16(*status).unwrap().into_response(),
        None => StatusCode::NO_CONTENT.into_response(),
    }
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
    let attempt = state.conversation_attempt.fetch_add(1, Ordering::SeqCst);
    if let Some((status, code)) = state.config.conversation_statuses.get(attempt) {
        return (
            StatusCode::from_u16(*status).unwrap(),
            Json(json!({"code":code})),
        )
            .into_response();
    }
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
    if let Some((status, code)) = &state.config.conversation_delete_status {
        return (
            StatusCode::from_u16(*status).unwrap(),
            Json(json!({"code":code})),
        )
            .into_response();
    }
    StatusCode::NO_CONTENT.into_response()
}

async fn responses(
    State(state): State<Arc<StateData>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let attachments = String::from_utf8_lossy(&body).contains("[Attachments:");
    let candidate = received("POST", "/api/v2/responses".into(), &headers, &body);
    record(&state, "POST", "/api/v2/responses".into(), headers, body);
    if attachments {
        if let Some(gate) = &state.config.attachment_gate {
            gate.acquire().await.unwrap().forget();
        }
    }
    if !state.config.header_delay.is_zero() {
        sleep(state.config.header_delay).await;
    }
    if attachments && !state.config.attachment_delay.is_zero() {
        sleep(state.config.attachment_delay).await;
    }
    if state.missing_turn_once.swap(false, Ordering::SeqCst) {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"code":"conversation_missing"})),
        )
            .into_response();
    }
    if let Some((status, code)) = &state.config.response_refusal {
        return (
            StatusCode::from_u16(*status).unwrap(),
            Json(json!({"error":{"code":code,"message":"private provider diagnostic"}})),
        )
            .into_response();
    }
    if state.config.busy_always || state.busy_once.swap(false, Ordering::SeqCst) {
        let mut response =
            (StatusCode::CONFLICT, Json(json!({"code":"session_busy"}))).into_response();
        if let Some(seconds) = state.config.busy_retry_after {
            response
                .headers_mut()
                .insert("retry-after", seconds.to_string().parse().unwrap());
        }
        return response;
    }
    if let Some(barrier) = &state.config.admission_barrier {
        barrier.wait().await;
    }
    {
        let mut admitted = state.admitted.lock().unwrap();
        if state.config.exact_admission {
            if let Some(original) = admitted.iter().find(|original| {
                original.authorization == candidate.authorization
                    && original.idempotency_key == candidate.idempotency_key
            }) {
                let status = if original.body == candidate.body {
                    "exact_duplicate"
                } else {
                    "agent_command_key_conflict"
                };
                return (StatusCode::CONFLICT, Json(json!({"code":status}))).into_response();
            }
        }
        admitted.push(candidate);
    }
    let mut frames = vec![
        "event: response.created\ndata: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_fake\"}}\n\n".to_owned(),
        "event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"item_id\":\"msg_1\",\"delta\":\"hello\"}\n\n".to_owned(),
    ];
    if state.config.in_progress_first {
        frames[0] = frames[0].replace("response.created", "response.in_progress");
    }
    frames[1] = format!(
        "data: {}\n\n",
        json!({"type":"response.output_text.delta", "item_id":"msg_1", "delta": state.config.text})
    );
    if state.config.unknown_event {
        frames.insert(1, "data: {\"type\":\"response.future\"}\n\n".into());
    }
    if state.config.malformed_frame {
        frames.push("data: {malformed}\n\n".to_owned());
    } else {
        frames.push("event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_fake\"}}\n\n".to_owned());
    }
    if let Some(custom) = &state.config.frames {
        frames = custom
            .iter()
            .map(|frame| format!("data: {frame}\n\n"))
            .collect();
    }
    if let Some(count) = state.config.disconnect_after {
        frames.truncate(count);
    }
    let delay = state.config.response_delay;
    let terminal_delay = state.config.terminal_delay;
    let terminal_gate = state.config.terminal_gate.clone();
    let last = frames.len().saturating_sub(1);
    let stream = stream::iter(frames.into_iter().enumerate()).then(move |(index, frame)| {
        let terminal_gate = terminal_gate.clone();
        async move {
            if index == 0 && !delay.is_zero() {
                sleep(delay).await;
            }
            if index == last {
                if !terminal_delay.is_zero() {
                    sleep(terminal_delay).await;
                }
                if let Some(gate) = terminal_gate {
                    gate.acquire().await.unwrap().forget();
                }
            }
            Ok::<Bytes, Infallible>(Bytes::from(frame))
        }
    });
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/event-stream")
        .body(Body::from_stream(stream))
        .unwrap()
}

async fn inspect_conversation(
    State(state): State<Arc<StateData>>,
    Path((binding, conversation)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    record(
        &state,
        "GET",
        format!("/api/v1/cmaeg/bindings/{binding}/conversations/{conversation}"),
        headers,
        Bytes::new(),
    );
    if let Some(view) = reset::inspection(&state, &binding, &conversation) {
        return Json(view).into_response();
    }
    match &state.config.inspection_agent {
        Some((agent_id, status)) => Json(json!({"binding_id":binding, "generation":1,
            "conversation_id":conversation,"agent_id":agent_id,"state":"ready","agent_status":status})).into_response(),
        None => (StatusCode::NOT_FOUND, Json(json!({"code":"binding_not_found"}))).into_response(),
    }
}

fn record(state: &StateData, method: &str, path: String, headers: HeaderMap, body: Bytes) {
    let request = received(method, path, &headers, &body);
    state.requests.lock().unwrap().push(request);
}

fn received(method: &str, path: String, headers: &HeaderMap, body: &[u8]) -> ReceivedRequest {
    ReceivedRequest {
        received_at: Instant::now(),
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
        content_type: headers
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned),
        body: String::from_utf8_lossy(body).into_owned(),
    }
}
