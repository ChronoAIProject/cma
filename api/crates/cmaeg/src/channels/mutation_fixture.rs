//! Independent management states, real CAS transports, and a barrier-controlled provider.
use super::management::ManagementState;
use crate::{
    auth::{Creator, CreatorAuthenticator},
    providers::{ProviderClient, ProviderRegistry},
    records::RecordClient,
};
use axum::{
    extract::{Path, State},
    http::{Method, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, put},
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use cmaeg_core::{
    channel::{ChannelDefinition, Secret},
    seal::MasterKeyRing,
};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::sync::Notify;

#[derive(Clone, Default)]
pub(super) struct Peer {
    pub pause: Arc<AtomicBool>,
    pub started: Arc<Notify>,
    pub resume: Arc<Notify>,
    pub finished: Arc<Notify>,
    pub calls: Arc<Mutex<Vec<String>>>,
    pub bound: Arc<Mutex<BTreeMap<String, Value>>>,
    pub rows: Arc<Mutex<BTreeMap<String, Value>>>,
    /// Lose the acknowledgement after committing this expected revision.
    pub lose_ack: Arc<AtomicU64>,
    /// Answer every binding DELETE with this definite refusal.
    pub refuse_delete: Arc<Mutex<Option<(StatusCode, &'static str)>>>,
    pub refuse_bind: Arc<Mutex<Option<StatusCode>>>,
}
pub(super) struct Fixture {
    pub peer: Peer,
    pub base: String,
    pub keys: Arc<MasterKeyRing>,
    pub custody: bool,
    mongo: Option<crate::replicas::mongo_fixture::Fixture>,
    server: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}
impl Fixture {
    pub async fn new(native: bool, custody: bool) -> Self {
        let peer = Peer::default();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let app = Router::new()
            .route("/bindings/{id}", put(provider).delete(provider))
            .route("/api/v1/gateway/channel-records", get(list))
            .route(
                "/api/v1/gateway/channel-records/{id}",
                put(save).get(load).delete(delete),
            )
            .with_state(peer.clone());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let mongo = if native {
            Some(crate::replicas::mongo_fixture::Fixture::new().await)
        } else {
            None
        };
        Self {
            peer,
            base,
            custody: custody && std::env::var_os("CMAEG_TEST_CUSTODY_DISABLED").is_none(),
            keys: Arc::new(
                MasterKeyRing::parse(&format!("1:{}", STANDARD.encode([74; 32]))).unwrap(),
            ),
            mongo,
            server,
        }
    }
    pub async fn state(&self) -> ManagementState {
        self.state_with_custody(self.custody).await
    }
    pub async fn state_with_custody(&self, custody: bool) -> ManagementState {
        let records = match &self.mongo {
            Some(mongo) => mongo.records(&self.base).await,
            None => RecordClient::new(&self.base, "fixture-service").unwrap(),
        };
        self.with_records(records, custody)
    }
    pub async fn state_with_tracking(
        &self,
        tracking: crate::worker_credentials::WorkerCredentials,
    ) -> ManagementState {
        let records = self
            .mongo
            .as_ref()
            .unwrap()
            .records_with_tracking(&self.base, Some(tracking))
            .await;
        self.with_records(records, self.custody)
    }
    fn with_records(&self, records: RecordClient, custody: bool) -> ManagementState {
        let records = records.with_channel_mutation_custody(custody);
        let registry = ProviderRegistry::parse(&json!([{"slug":"cma_codex","base_url":self.base,"kind":"responses_http","paths":{"agent_card":"/card","binding":"/bindings/{binding_id}","conversation":"/bindings/{binding_id}/conversations/{conversation_id}","responses":"/responses"},"max_inflight":8}]).to_string()).unwrap();
        ManagementState::new(
            Arc::new(
                ProviderClient::new(registry, Duration::from_secs(1), Duration::from_secs(10), 8)
                    .unwrap(),
            ),
            Arc::new(records),
            Arc::new(CreatorAuthenticator::new(&self.base).unwrap()),
            crate::ManagementConfig {
                public_url: "https://gateway.example".into(),
                public_api_url: "https://gateway.example".into(),
                public_api_aliases: vec![],
                advertise_public_api_url: true,
                console_public_url: None,
                master_keys: self.keys.clone(),
                cma_base_url: self.base.clone(),
                cma_service_token: Secret::new("fixture-service"),
                nyxid_base_url: self.base.clone(),
                tools_nyxid_slug: "cmaeg".into(),
            },
            Default::default(),
        )
    }
    pub fn body(&self) -> Value {
        let definition: ChannelDefinition = serde_json::from_str(include_str!(
            "../../../cmaeg-core/tests/fixtures/channel.json"
        ))
        .unwrap();
        let mut profile = definition.consumer(None).unwrap().profile.clone();
        profile.metadata.owner.subject = "alice".into();
        json!({"schema_version":1,"record_id":uuid::Uuid::new_v4().to_string(),"name":"Original","profile":profile,"provider":{"slug":"cma_codex"},
            "sources":[{"type":"nyxid_relay","issuer":"https://nyx.example","key_id":"route-key","route_ids":[],"platform":"telegram","admission":{"type":"all"}}],
            "partition":{"version":1,"key":"conversation_and_sender"},"reply":{"auto_final":true,"ack_after_ms":3000,"sinks":[{"type":"source"}]},"credentials":{"agent_key":"fixture-agent-key","channel_key":"fixture-channel-key"}})
    }
    pub fn update(&self, body: &Value, version: u64, name: &str) -> super::ChannelUpdateRequest {
        let mut body = body.clone();
        body.as_object_mut().unwrap().remove("record_id");
        body["expected_version"] = version.into();
        body["name"] = name.into();
        body["profile"]["metadata"]["name"] = name.into();
        serde_json::from_value(body).unwrap()
    }
    pub async fn cleanup(mut self) {
        if let Some(mongo) = self.mongo.take() {
            mongo.cleanup().await;
        }
    }
}
pub(super) fn owner(subject: &str, organization: bool) -> Creator {
    Creator {
        subject: subject.into(),
        token_hash: "fixture-auth".into(),
        organization,
    }
}
pub(super) async fn wait(signal: &Notify) {
    tokio::time::timeout(Duration::from_secs(5), signal.notified())
        .await
        .expect("fixture barrier deadline");
}
async fn provider(
    State(peer): State<Peer>,
    Path(id): Path<String>,
    method: Method,
    body: Option<Json<Value>>,
) -> Response {
    peer.calls.lock().unwrap().push(method.to_string());
    if method == Method::DELETE {
        if let Some((status, code)) = *peer.refuse_delete.lock().unwrap() {
            return (status, Json(json!({"code": code}))).into_response();
        }
    } else if let Some(status) = *peer.refuse_bind.lock().unwrap() {
        return (status, Json(json!({"code": "invalid_binding"}))).into_response();
    }
    let value = body.map(|Json(value)| value).unwrap_or(Value::Null);
    if peer.pause.swap(false, Ordering::SeqCst) {
        let (done, wait) = tokio::sync::oneshot::channel();
        let remote = peer.clone();
        let effect_id = id.clone();
        let effect_method = method.clone();
        tokio::spawn(async move {
            remote.started.notify_one();
            remote.resume.notified().await;
            apply(&remote, &effect_id, &effect_method, value);
            remote.finished.notify_one();
            let _ = done.send(());
        });
        wait.await.unwrap();
    } else {
        apply(&peer, &id, &method, value);
    }
    Json(if method == Method::DELETE {
        json!({})
    } else {
        json!({"binding_id":id,"level":"l1","honoured":[],"refused":[]})
    })
    .into_response()
}
fn apply(peer: &Peer, id: &str, method: &Method, value: Value) {
    if method == Method::DELETE {
        peer.bound.lock().unwrap().remove(id);
    } else {
        peer.bound.lock().unwrap().insert(id.into(), value);
    }
}
async fn save(
    State(peer): State<Peer>,
    Path(id): Path<String>,
    Json(mut body): Json<Value>,
) -> Response {
    let mut rows = peer.rows.lock().unwrap();
    let current = rows.get(&id).and_then(|row| row["version"].as_u64());
    if current != body["expected_version"].as_u64() {
        return (StatusCode::CONFLICT, Json(json!({"actual":current}))).into_response();
    }
    let version = current.unwrap_or(0) + 1;
    body["version"] = version.into();
    rows.insert(id, body);
    if current.is_some_and(|version| {
        peer.lose_ack
            .compare_exchange(version, 0, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }) {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    Json(json!({"version":version})).into_response()
}
async fn load(State(peer): State<Peer>, Path(id): Path<String>) -> Response {
    match peer.rows.lock().unwrap().get(&id) {
        Some(row) => Json(row.clone()).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
async fn delete(
    State(peer): State<Peer>,
    Path(id): Path<String>,
    axum::extract::Query(query): axum::extract::Query<BTreeMap<String, String>>,
) -> Response {
    let mut rows = peer.rows.lock().unwrap();
    let Some(row) = rows.get(&id) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if query
        .get("expected_version")
        .and_then(|v| v.parse::<u64>().ok())
        != row["version"].as_u64()
    {
        return (StatusCode::CONFLICT, Json(json!({"actual":row["version"]}))).into_response();
    }
    rows.remove(&id);
    StatusCode::NO_CONTENT.into_response()
}

async fn list(State(peer): State<Peer>) -> Json<Value> {
    let rows = peer.rows.lock().unwrap();
    let items: Vec<_> = rows
        .iter()
        .map(|(id, row)| json!({"record_id": id, "owner": row["owner"], "version": row["version"]}))
        .collect();
    Json(json!({"items": items}))
}
