//! Local Kubernetes transport with real API objects and assignment receipts.
use axum::{
    body::{to_bytes, Body},
    extract::{
        ws::{Message, WebSocketUpgrade},
        State,
    },
    http::{Request, StatusCode},
    response::{IntoResponse, Response},
    routing::any,
    Json, Router,
};
use layer3_adapter::runtime_bootstrap::{RuntimeBootstrapReceipt, RuntimeBootstrapRequest};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

pub(super) const PREFIX: &str = "/apis/extensions.agents.x-k8s.io/v1beta1/namespaces/cma-test";
pub(super) const MEMBER: &str =
    "/apis/agents.x-k8s.io/v1beta1/namespaces/cma-test/sandboxes/warm-member";
pub(super) const POD: &str = "/api/v1/namespaces/cma-test/pods/warm-member";
pub(super) const VOLUME: &str =
    "/api/v1/namespaces/cma-test/persistentvolumeclaims/home-warm-member";

type DeleteCheck = Box<dyn Fn() -> futures_util::future::BoxFuture<'static, ()> + Send>;

#[derive(Default)]
pub(super) struct ApiState {
    pub rows: BTreeMap<String, Value>,
    pub calls: Vec<(String, String)>,
    pub assignments: Vec<RuntimeBootstrapRequest>,
    pub assignment_boundaries: Vec<usize>,
    pub before_delete: Option<DeleteCheck>,
    pub lose_member_delete: bool,
    version: u64,
}

pub(super) struct Api {
    pub client: kube::Client,
    pub state: Arc<Mutex<ApiState>>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Api {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Api {
    pub async fn new() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let state = Arc::new(Mutex::new(ApiState::default()));
        let router = Router::new()
            .route(&format!("{POD}/exec"), any(assign))
            .fallback(request)
            .with_state(state.clone());
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let client = kube::Client::try_from(kube::Config::new(
            format!("http://{address}").parse().unwrap(),
        ))
        .unwrap();
        Self {
            client,
            state,
            task,
        }
    }
}

async fn request(State(state): State<Arc<Mutex<ApiState>>>, request: Request<Body>) -> Response {
    let (parts, body) = request.into_parts();
    let bytes = to_bytes(body, 1024 * 1024).await.unwrap();
    let body: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    if parts.method == "DELETE" {
        let check = state.lock().unwrap().before_delete.as_ref().map(|f| f());
        if let Some(check) = check {
            check.await;
        }
    }
    let mut state = state.lock().unwrap();
    let path = parts.uri.path();
    state.calls.push((parts.method.to_string(), path.into()));
    let missing = json!({"apiVersion":"v1","kind":"Status","status":"Failure","reason":"NotFound","code":404});
    let (code, value) = match parts.method.as_str() {
        "GET" if state.rows.contains_key(path) => (200, state.rows[path].clone()),
        "GET"
            if [
                "pods",
                "sandboxes",
                "sandboxclaims",
                "sandboxwarmpools",
                "persistentvolumeclaims",
            ]
            .iter()
            .any(|resource| path.ends_with(&format!("/{resource}"))) =>
        {
            let url = url::Url::parse(&format!("http://fixture{}", parts.uri)).unwrap();
            let selector = url
                .query_pairs()
                .find(|(key, _)| key == "labelSelector")
                .map(|(_, value)| value.into_owned())
                .unwrap_or_default();
            let items: Vec<_> = state
                .rows
                .iter()
                .filter(|(key, row)| {
                    key.starts_with(&format!("{path}/"))
                        && selector.split(',').filter(|s| !s.is_empty()).all(|term| {
                            let (key, value) = term.split_once('=').unwrap();
                            row["metadata"]["labels"][key] == value
                        })
                })
                .map(|(_, row)| row.clone())
                .collect();
            let (version, kind) = match path.rsplit('/').next().unwrap() {
                "pods" => ("v1", "PodList"),
                "persistentvolumeclaims" => ("v1", "PersistentVolumeClaimList"),
                "sandboxes" => ("agents.x-k8s.io/v1beta1", "SandboxList"),
                "sandboxclaims" => ("extensions.agents.x-k8s.io/v1beta1", "SandboxClaimList"),
                "sandboxwarmpools" => ("extensions.agents.x-k8s.io/v1beta1", "SandboxWarmPoolList"),
                _ => unreachable!(),
            };
            (
                200,
                json!({"apiVersion":version,"kind":kind,"metadata":{"resourceVersion":state.version.to_string()},"items":items}),
            )
        }
        "GET" => (404, missing),
        "POST" => {
            let key = format!("{path}/{}", body["metadata"]["name"].as_str().unwrap());
            assert!(!state.rows.contains_key(&key));
            state.version += 1;
            let mut row = body;
            row["metadata"]["uid"] = json!(format!("fixture-{}", state.version));
            row["metadata"]["resourceVersion"] = json!(state.version.to_string());
            row["metadata"]["generation"] = json!(1);
            state.rows.insert(key, row.clone());
            (201, row)
        }
        "PATCH" => {
            let mut row = state.rows[path].clone();
            assert_eq!(body["metadata"]["uid"], row["metadata"]["uid"]);
            assert_eq!(
                body["metadata"]["resourceVersion"],
                row["metadata"]["resourceVersion"]
            );
            merge(&mut row, &body);
            state.version += 1;
            row["metadata"]["resourceVersion"] = json!(state.version.to_string());
            if body.get("spec").is_some() {
                row["metadata"]["generation"] =
                    json!(row["metadata"]["generation"].as_u64().unwrap() + 1);
            }
            state.rows.insert(path.into(), row.clone());
            (200, row)
        }
        "DELETE" if state.before_delete.is_some() && state.rows.contains_key(path) => {
            let metadata = &state.rows[path]["metadata"];
            assert_eq!(
                body["preconditions"],
                json!({"uid":metadata["uid"],"resourceVersion":metadata["resourceVersion"]})
            );
            state.rows.remove(path);
            if path == MEMBER {
                state.rows.remove(POD);
                state.rows.remove(VOLUME);
                if std::mem::take(&mut state.lose_member_delete) {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({"message":"fixture lost delete receipt"})),
                    )
                        .into_response();
                }
            }
            (
                200,
                json!({"apiVersion":"v1","kind":"Status","status":"Success","code":200}),
            )
        }
        "DELETE" => (
            403,
            json!({"apiVersion":"v1","kind":"Status","status":"Failure","reason":"Forbidden","code":403}),
        ),
        _ => panic!("unexpected fixture method"),
    };
    (StatusCode::from_u16(code).unwrap(), Json(value)).into_response()
}

fn merge(target: &mut Value, patch: &Value) {
    if let Some(fields) = patch.as_object() {
        if !target.is_object() {
            *target = json!({});
        }
        for (key, value) in fields {
            if value.is_null() {
                target.as_object_mut().unwrap().remove(key);
            } else {
                merge(&mut target[key], value);
            }
        }
    } else {
        *target = patch.clone();
    }
}

async fn assign(State(state): State<Arc<Mutex<ApiState>>>, ws: WebSocketUpgrade) -> Response {
    ws.protocols(["v5.channel.k8s.io"])
        .on_upgrade(move |mut socket| async move {
            let mut input = Vec::new();
            while let Some(message) = socket.recv().await {
                let Message::Binary(bytes) = message.unwrap() else {
                    continue;
                };
                if bytes.as_ref() == [255, 0] {
                    break;
                }
                assert_eq!(bytes[0], 0);
                input.extend_from_slice(&bytes[1..]);
            }
            let request: RuntimeBootstrapRequest = serde_json::from_slice(&input).unwrap();
            let receipt = RuntimeBootstrapReceipt::for_request(&request).unwrap();
            {
                let mut state = state.lock().unwrap();
                let boundary = state.calls.len();
                state.assignment_boundaries.push(boundary);
                state.assignments.push(request);
            }
            let mut stdout = vec![1];
            stdout.extend(serde_json::to_vec(&receipt).unwrap());
            socket.send(Message::Binary(stdout.into())).await.unwrap();
            let mut status = vec![3];
            status.extend(
                serde_json::to_vec(&json!({"apiVersion":"v1","kind":"Status","status":"Success"}))
                    .unwrap(),
            );
            let _ = socket.send(Message::Binary(status.into())).await;
            let _ = socket.send(Message::Close(None)).await;
        })
        .into_response()
}
