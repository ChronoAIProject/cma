use axum::{
    body::Body,
    extract::{Request, State},
    response::Response,
    Router,
};
use cmaeg::providers::{ProviderClient, ProviderRegistry, RecoveryIdentity};
use cmaeg_core::{channel::Secret, ids::ProviderSlug};
use futures_util::{stream, StreamExt};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    convert::Infallible,
    sync::{Arc, Mutex},
    time::Duration,
};

pub fn provider() -> ProviderSlug {
    ProviderSlug::new("cma_codex").unwrap()
}
pub fn key() -> Secret {
    Secret::new("fixture-current-agent-key")
}
pub fn identity() -> RecoveryIdentity {
    RecoveryIdentity {
        response_id: "resp_accepted".into(),
        native_agent_id: None,
        metadata: BTreeMap::from([("cmaeg_event_id".into(), "event-frozen".into())]),
    }
}
pub fn snapshot(status: &str) -> Value {
    json!({"id":"resp_accepted","object":"response","status":status,
        "conversation":{"id":"agt_native"},"metadata":identity().metadata,"output":[]})
}
pub fn lifecycle(seq: u64, kind: &str, status: &str) -> String {
    frame(
        Some(&seq.to_string()),
        json!({"type":kind,"sequence_number":seq,"response":snapshot(status)}),
    )
}
pub fn frame(id: Option<&str>, event: Value) -> String {
    let prefix = id.map_or(String::new(), |id| format!("id: {id}\n"));
    format!(
        "{prefix}event: {}\ndata: {event}\n\n",
        event["type"].as_str().unwrap()
    )
}
pub fn frames() -> String {
    lifecycle(0, "response.created", "queued")
        + &lifecycle(1, "response.in_progress", "in_progress")
        + &frame(
            Some("2"),
            json!({"type":"response.output_text.delta","sequence_number":2,"response_id":"resp_accepted","item_id":"msg_1","delta":"hello"}),
        )
        + &lifecycle(3, "response.completed", "completed")
}
#[derive(Clone)]
pub struct Config {
    pub snapshot: Value,
    pub frames: String,
    pub header: Option<String>,
    pub status: u16,
    pub location: Option<String>,
    pub idle: bool,
    pub header_delay: Duration,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            snapshot: snapshot("completed"),
            frames: frames(),
            header: Some("resp_accepted".into()),
            status: 200,
            location: None,
            idle: false,
            header_delay: Duration::ZERO,
        }
    }
}
#[derive(Debug, Clone)]
pub struct Seen {
    pub method: String,
    pub uri: String,
    pub authorization: String,
}
pub struct Server {
    pub base: String,
    pub config: Arc<Mutex<Config>>,
    pub seen: Arc<Mutex<Vec<Seen>>>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Server {
    pub async fn new(config: Config) -> Self {
        let config = Arc::new(Mutex::new(config));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let state = (config.clone(), seen.clone());
        let router = Router::new().fallback(handle).with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Self {
            base,
            config,
            seen,
            task,
        }
    }
    pub fn client(&self, recovery: bool, timeout: Duration) -> ProviderClient {
        let mut value = json!([{"slug":"cma_codex","base_url":self.base,"kind":"responses_http","max_inflight":1,
            "paths":{"agent_card":"/card","binding":"/bindings/{binding_id}","conversation":"/bindings/{binding_id}/conversations/{conversation_id}","responses":"/api/v2/responses"}}]);
        if recovery {
            value[0]["paths"]["recovery"] = json!({"response":"/api/v2/responses/{response_id}","events":"/api/v2/responses/{response_id}/events"});
        }
        ProviderClient::new(
            ProviderRegistry::parse(&value.to_string()).unwrap(),
            Duration::from_secs(1),
            timeout,
            1,
        )
        .unwrap()
    }
}
type StateData = (Arc<Mutex<Config>>, Arc<Mutex<Vec<Seen>>>);
async fn handle(State((config, seen)): State<StateData>, request: Request) -> Response {
    seen.lock().unwrap().push(Seen {
        method: request.method().to_string(),
        uri: request.uri().to_string(),
        authorization: request
            .headers()
            .get("authorization")
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned(),
    });
    let config = config.lock().unwrap().clone();
    tokio::time::sleep(config.header_delay).await;
    let mut builder = Response::builder().status(config.status);
    if let Some(location) = config.location {
        builder = builder.header("location", location);
    }
    if config.status != 200 {
        return builder
            .header("retry-after", "2")
            .body(Body::from(r#"{"error":{"code":"authority_unavailable"}}"#))
            .unwrap();
    }
    if request.uri().path().ends_with("/events") {
        builder = builder.header("content-type", "text/event-stream");
        if let Some(header) = config.header {
            builder = builder.header("x-cma-response-id", header);
        }
        let body = if config.idle {
            Body::from_stream(
                stream::once(async { Ok::<_, Infallible>(": keepalive\n\n") })
                    .chain(stream::pending()),
            )
        } else {
            Body::from(config.frames)
        };
        builder.body(body).unwrap()
    } else {
        builder
            .header("content-type", "application/json")
            .body(Body::from(config.snapshot.to_string()))
            .unwrap()
    }
}
