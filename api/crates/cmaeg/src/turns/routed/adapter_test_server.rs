use crate::providers::{ProviderClient, ProviderRegistry};
use axum::{
    body::{to_bytes, Body},
    extract::{Request, State},
    response::Response,
    Router,
};
use futures_util::{stream, StreamExt};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    convert::Infallible,
    sync::{Arc, Mutex},
    time::Duration,
};

#[derive(Clone)]
pub struct Config {
    pub snapshot_body: Option<Value>,
    pub replay_frames: Option<Vec<Value>>,
    pub mirror_replay_metadata: bool,
    pub event_context: Option<Value>,
    pub replay_idle: bool,
    pub replay_delay: Duration,
    pub replay_barrier: Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>,
    pub replay_frame_delay: Duration,
    pub replay_tail: String,
    pub post_status: u16,
    pub post_error_body: Option<String>,
    pub post_error_content_type: String,
    pub post_error_retry_after: Option<String>,
    pub post_error_response_id: bool,
    pub post_error_idle: bool,
    pub post_error_transport_failure: bool,
    pub post_idle: bool,
    pub post_delay: Duration,
    pub native_agent: String,
    pub corrupt_acceptance: bool,
    pub acceptance_fault: Option<&'static str>,
    pub conversation_status: u16,
    pub conversation_delay: Duration,
    pub on_conversation: Option<Arc<dyn Fn() + Send + Sync>>,
    pub on_post: Option<Arc<dyn Fn() + Send + Sync>>,
    pub on_snapshot: Option<Arc<dyn Fn() + Send + Sync>>,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            snapshot_body: None,
            replay_frames: None,
            mirror_replay_metadata: false,
            event_context: None,
            replay_idle: false,
            replay_delay: Duration::ZERO,
            replay_barrier: None,
            replay_frame_delay: Duration::ZERO,
            replay_tail: String::new(),
            post_status: 200,
            post_error_body: None,
            post_error_content_type: "application/json".into(),
            post_error_retry_after: None,
            post_error_response_id: false,
            post_error_idle: false,
            post_error_transport_failure: false,
            post_idle: false,
            post_delay: Duration::ZERO,
            native_agent: "agt_native".into(),
            corrupt_acceptance: false,
            acceptance_fault: None,
            conversation_status: 201,
            conversation_delay: Duration::ZERO,
            on_conversation: None,
            on_post: None,
            on_snapshot: None,
        }
    }
}
#[derive(Clone)]
pub struct Seen {
    pub method: String,
    pub uri: String,
    pub bearer: String,
    pub idempotency: Option<String>,
    pub body: Vec<u8>,
}
pub struct Memory {
    pub config: Config,
    pub seen: Vec<Seen>,
    pub metadata: BTreeMap<String, String>,
    pub conversations: BTreeSet<String>,
}
pub struct HttpServer {
    pub base: String,
    pub memory: Arc<Mutex<Memory>>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for HttpServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl HttpServer {
    pub async fn start() -> Self {
        let memory = Arc::new(Mutex::new(Memory {
            config: Config::default(),
            seen: vec![],
            metadata: BTreeMap::new(),
            conversations: BTreeSet::new(),
        }));
        let router = Router::new().fallback(handle).with_state(memory.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Self { base, memory, task }
    }
    pub fn client(&self, recovery: bool, timeout: Duration) -> Arc<ProviderClient> {
        let mut value = json!([{"slug":"cma_codex","base_url":self.base,"kind":"responses_http","max_inflight":1,
            "paths":{"agent_card":"/card","binding":"/bindings/{binding_id}","conversation":"/bindings/{binding_id}/conversations/{conversation_id}","responses":"/api/v2/responses"}}]);
        if recovery {
            value[0]["paths"]["recovery"] = json!({"response":"/api/v2/responses/{response_id}","events":"/api/v2/responses/{response_id}/events"});
        }
        Arc::new(
            ProviderClient::new(
                ProviderRegistry::parse(&value.to_string()).unwrap(),
                Duration::from_secs(1),
                timeout,
                1,
            )
            .unwrap(),
        )
    }
    pub fn seen(&self) -> Vec<Seen> {
        self.memory.lock().unwrap().seen.clone()
    }
}
async fn handle(State(memory): State<Arc<Mutex<Memory>>>, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    let body = to_bytes(body, 2 * 1024 * 1024).await.unwrap().to_vec();
    let seen = Seen {
        method: parts.method.to_string(),
        uri: parts.uri.to_string(),
        bearer: parts
            .headers
            .get("authorization")
            .unwrap()
            .to_str()
            .unwrap()
            .into(),
        idempotency: parts
            .headers
            .get("idempotency-key")
            .map(|value| value.to_str().unwrap().into()),
        body,
    };
    let post = parts.method == axum::http::Method::POST;
    let (config, metadata) = {
        let mut memory = memory.lock().unwrap();
        if post {
            let value: Value = serde_json::from_slice(&seen.body).unwrap();
            memory.metadata = serde_json::from_value(value["metadata"].clone()).unwrap();
        }
        memory.seen.push(seen);
        (memory.config.clone(), memory.metadata.clone())
    };
    if parts.method == axum::http::Method::PUT {
        tokio::time::sleep(config.conversation_delay).await;
        if let Some(callback) = &config.on_conversation {
            callback();
        }
        let status = if (200..300).contains(&config.conversation_status)
            && !memory
                .lock()
                .unwrap()
                .conversations
                .insert(parts.uri.path().into())
        {
            200
        } else {
            config.conversation_status
        };
        return Response::builder()
            .status(status)
            .header("content-type", "application/json")
            .body(Body::from("{}"))
            .unwrap();
    }
    if post {
        if let Some(callback) = &config.on_post {
            callback();
        }
        tokio::time::sleep(config.post_delay).await;
    }
    if post && config.post_status != 200 {
        let mut response = Response::builder()
            .status(config.post_status)
            .header("content-type", config.post_error_content_type);
        if let Some(value) = config.post_error_retry_after {
            response = response.header("retry-after", value);
        }
        if config.post_error_response_id {
            response = response.header("x-cma-response-id", "resp_accepted");
        }
        let body = config.post_error_body.unwrap_or_else(|| {
                json!({"error":{"code":if config.post_status==400 {"invalid_json"} else {"response_unavailable"}}}).to_string()
            });
        let body = if config.post_error_idle {
            Body::from_stream(
                stream::once(async move { Ok::<_, Infallible>(body) }).chain(stream::pending()),
            )
        } else if config.post_error_transport_failure {
            Body::from_stream(stream::iter([
                Ok(body),
                Err(std::io::Error::other("fixture error body truncated")),
            ]))
        } else {
            Body::from(body)
        };
        return response.body(body).unwrap();
    }
    if parts.method == axum::http::Method::GET
        && (parts.uri.path().starts_with("/bindings/")
            || parts.uri.path().starts_with("/api/v1/cmaeg/bindings/"))
        && parts.uri.path().contains("/conversations/")
        && parts.uri.path().contains("/events/")
    {
        return Response::builder()
            .status(if config.event_context.is_some() {
                200
            } else {
                404
            })
            .header("content-type", "application/json")
            .body(Body::from(
                config.event_context.unwrap_or(Value::Null).to_string(),
            ))
            .unwrap();
    }
    let snapshot = |status: &str| json!({"id":"resp_accepted","object":"response","status":status,"conversation":{"id":config.native_agent},"metadata":metadata,"output":[]});
    if !post && !parts.uri.path().ends_with("/events") {
        if let Some(callback) = config.on_snapshot.as_ref() {
            callback();
        }
        return Response::builder()
            .header("content-type", "application/json")
            .body(Body::from(
                config
                    .snapshot_body
                    .clone()
                    .unwrap_or_else(|| snapshot("completed"))
                    .to_string(),
            ))
            .unwrap();
    }
    if !post {
        if let Some((entered, release)) = &config.replay_barrier {
            entered.notify_one();
            release.notified().await;
        }
        tokio::time::sleep(config.replay_delay).await;
    }
    let body = if post && config.post_idle {
        Body::from_stream(
            stream::once(async { Ok::<_, Infallible>(": waiting\n\n") }).chain(stream::pending()),
        )
    } else {
        let after = parts
            .uri
            .query()
            .and_then(|query| query.strip_prefix("starting_after="))
            .and_then(|value| value.parse::<u64>().ok());
        let mut frames = String::new();
        for (sequence, kind, status) in [
            (0, "response.created", "queued"),
            (1, "response.in_progress", "in_progress"),
            (2, "response.completed", "completed"),
        ] {
            if after.is_some_and(|after| sequence <= after) {
                continue;
            }
            let mut response = snapshot(status);
            if post && config.corrupt_acceptance {
                response["metadata"] = json!({"tampered":"true"});
            }
            let event = json!({"type":kind,"sequence_number":sequence,"response":response});
            let id = if post && config.acceptance_fault == Some("sequence") {
                1
            } else {
                sequence
            };
            frames.push_str(&format!("id: {id}\nevent: {kind}\ndata: {event}\n\n"));
            if post {
                break;
            }
        }
        if !post {
            if let Some(events) = config.replay_frames {
                frames.clear();
                for mut event in events {
                    if config.mirror_replay_metadata && event["response"].is_object() {
                        event["response"]["metadata"] = json!(metadata);
                    }
                    let sequence = event["sequence_number"].as_u64().unwrap();
                    if after.is_some_and(|after| sequence <= after) {
                        continue;
                    }
                    frames.push_str(&format!("id: {sequence}\ndata: {event}\n\n"));
                }
            }
            frames.push_str(&config.replay_tail);
        }
        if !post && !config.replay_frame_delay.is_zero() {
            let parts = frames
                .split_inclusive("\n\n")
                .map(str::to_owned)
                .collect::<Vec<_>>();
            let delay = config.replay_frame_delay;
            let paced = stream::iter(parts).then(move |part| async move {
                tokio::time::sleep(delay).await;
                Ok::<_, Infallible>(part)
            });
            Body::from_stream(
                paced.chain(
                    stream::pending::<Result<String, Infallible>>()
                        .take(usize::from(config.replay_idle)),
                ),
            )
        } else if post || config.replay_idle {
            Body::from_stream(
                stream::once(async move { Ok::<_, Infallible>(frames) }).chain(stream::pending()),
            )
        } else {
            Body::from(frames)
        }
    };
    let mut response = Response::builder().header(
        "content-type",
        if post && config.acceptance_fault == Some("content_type") {
            "application/json"
        } else {
            "text/event-stream"
        },
    );
    if !post || config.acceptance_fault != Some("missing_identity") {
        response = response.header("x-cma-response-id", "resp_accepted");
    }
    if post && config.acceptance_fault == Some("duplicate_identity") {
        response = response.header("x-cma-response-id", "resp_accepted");
    }
    response.body(body).unwrap()
}
