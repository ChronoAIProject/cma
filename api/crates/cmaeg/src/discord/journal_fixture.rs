use super::{discovery::DiscordApi, journal::DurableJournal, record_store::Store, GatewayError};
use crate::{channels::routing_fixture, records::RecordClient};
use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use cmaeg_core::{
    channel::ChannelDefinition,
    seal::{seal_blob, MasterKeyRing},
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering},
        Arc, Mutex,
    },
};

pub const TOKEN: &str = "discord-private-fixture";
#[derive(Clone)]
pub struct Row {
    pub blob: Vec<u8>,
    pub version: u64,
    pub owner: String,
    pub expiry: Option<i64>,
}
type OnSaved = Arc<dyn Fn(&str) + Send + Sync>;
/// Signals entry into the channel-record read, then holds it until notified.
pub type ChannelGate = (Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>);

pub struct Memory {
    pub rows: Mutex<BTreeMap<String, Row>>,
    pub channel: Mutex<Vec<u8>>,
    pub now: Arc<AtomicI64>,
    pub user: Mutex<Value>,
    pub reaction_users: Mutex<BTreeMap<String, Value>>,
    pub user_rate_limited: AtomicBool,
    pub user_calls: AtomicUsize,
    pub gateway: Mutex<Value>,
    pub discord_channels: Mutex<BTreeMap<String, Value>>,
    pub channel_calls: Mutex<Vec<String>>,
    pub channel_status: Mutex<StatusCode>,
    pub channel_elapsed_ms: AtomicI64,
    pub fault: Mutex<Option<(String, bool)>>,
    pub on_saved: Mutex<Option<OnSaved>>,
    pub channel_gate: Mutex<Option<ChannelGate>>,
    pub next_write_gate: Mutex<Option<ChannelGate>>,
}
pub struct Fixture {
    pub memory: Arc<Memory>,
    pub(super) store: Store,
    pub api: DiscordApi,
    pub record: String,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Fixture {
    pub fn records(&self) -> RecordClient {
        self.store.channels.clone()
    }
    pub fn keys(&self) -> Arc<MasterKeyRing> {
        self.store.keys.clone()
    }
    pub async fn new() -> Self {
        let keys =
            Arc::new(MasterKeyRing::parse(&format!("1:{}", STANDARD.encode([7; 32]))).unwrap());
        let definition = definition();
        let now = Arc::new(AtomicI64::new(
            time::OffsetDateTime::now_utc().unix_timestamp() * 1000,
        ));
        let memory = Arc::new(Memory {
            rows: Mutex::new(BTreeMap::new()),
            channel: Mutex::new(
                seal_blob(
                    &keys,
                    &serde_json::to_vec(&definition.to_blob_json()).unwrap(),
                )
                .unwrap(),
            ),
            now: now.clone(),
            user: Mutex::new(json!({"id":"77","bot":true})),
            reaction_users: Mutex::default(),
            user_rate_limited: AtomicBool::new(false),
            user_calls: AtomicUsize::new(0),
            gateway: Mutex::new(
                json!({"url":"wss://gateway.discord.gg/","shards":1,"session_start_limit":{"total":10,"remaining":10,"reset_after":86_400_000,"max_concurrency":1}}),
            ),
            fault: Mutex::new(None),
            on_saved: Mutex::new(None),
            channel_gate: Mutex::new(None),
            next_write_gate: Mutex::new(None),
            discord_channels: Mutex::default(),
            channel_calls: Mutex::default(),
            channel_status: Mutex::new(StatusCode::OK),
            channel_elapsed_ms: AtomicI64::new(0),
        });
        let app = Router::new()
            .route("/api/v1/gateway/channel-records/{id}", get(channel))
            .route(
                "/api/v1/gateway/connection-records/{id}",
                get(load).put(save),
            )
            .route(
                "/api/v1/gateway/interaction-records/{id}",
                get(load).put(save),
            )
            .route("/api/v1/gateway/interaction-records", get(list))
            .route("/api/v10/users/@me", get(user))
            .route("/api/v10/users/{id}", get(reaction_user))
            .route("/api/v10/gateway/bot", get(gateway))
            .route("/api/v10/channels/{id}", get(discord_channel))
            .with_state(memory.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let mut store = Store::new(
            RecordClient::new(&base, "record-private-fixture").unwrap(),
            keys,
        );
        store.clock = Arc::new(move || now.load(Ordering::SeqCst));
        Self {
            memory,
            store,
            api: DiscordApi::fixture(&format!("{base}/api/v10/")),
            record: definition.record_id.to_string(),
            task,
        }
    }
    pub async fn acquire(&self) -> Result<Arc<DurableJournal>, GatewayError> {
        self.acquire_observed(Arc::new(crate::turns::observer::NoopObserver))
            .await
    }
    pub async fn acquire_observed(
        &self,
        activity: Arc<dyn crate::turns::observer::ActivityObserver>,
    ) -> Result<Arc<DurableJournal>, GatewayError> {
        DurableJournal::acquire_store(self.store.clone(), &self.record, self.api.clone(), activity)
            .await
    }
    pub fn advance(&self, millis: i64) {
        self.memory.now.fetch_add(millis, Ordering::SeqCst);
    }
    pub fn replace(&self, definition: &ChannelDefinition) {
        *self.memory.channel.lock().unwrap() = seal_blob(
            &self.store.keys,
            &serde_json::to_vec(&definition.to_blob_json()).unwrap(),
        )
        .unwrap();
    }
    pub fn message_id(&self) -> String {
        (((self.memory.now.load(Ordering::SeqCst) - 1_420_070_400_000) as u64) << 22).to_string()
    }
    pub fn fail(&self, id: &str, after_commit: bool) {
        *self.memory.fault.lock().unwrap() = Some((id.into(), after_commit));
    }
}
pub fn definition() -> ChannelDefinition {
    let mut value = routing_fixture::routed();
    value["record_id"] = "11111111-1111-4111-8111-111111111111".into();
    value.as_object_mut().unwrap().remove("routing");
    value["secrets"]
        .as_object_mut()
        .unwrap()
        .remove("consumer_keys");
    value["sources"] =
        json!([{"type":"discord","bot_id":"77","intents":37377,"admission":{"type":"all"}}]);
    value["secrets"]["discord_bot_token"] = TOKEN.into();
    let definition = routing_fixture::definition(value);
    definition.validate().unwrap();
    definition
}
async fn user(State(state): State<Arc<Memory>>, headers: HeaderMap) -> Response {
    state.user_calls.fetch_add(1, Ordering::SeqCst);
    if state.user_rate_limited.load(Ordering::SeqCst) {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            [("retry-after", "120")],
            Json(json!({"retry_after":90})),
        )
            .into_response();
    }
    if headers.get("authorization").and_then(|v| v.to_str().ok()) != Some(&format!("Bot {TOKEN}")) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    Json(state.user.lock().unwrap().clone()).into_response()
}
async fn gateway(State(state): State<Arc<Memory>>) -> Response {
    Json(state.gateway.lock().unwrap().clone()).into_response()
}
async fn discord_channel(
    State(state): State<Arc<Memory>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if headers.get("authorization").and_then(|v| v.to_str().ok()) != Some(&format!("Bot {TOKEN}")) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    state.channel_calls.lock().unwrap().push(id.clone());
    state.now.fetch_add(
        state.channel_elapsed_ms.load(Ordering::SeqCst),
        Ordering::SeqCst,
    );
    let status = *state.channel_status.lock().unwrap();
    if status == StatusCode::TOO_MANY_REQUESTS {
        return (
            status,
            [("retry-after", "120")],
            Json(json!({"retry_after":120})),
        )
            .into_response();
    }
    match state.discord_channels.lock().unwrap().get(&id).cloned() {
        Some(value) => (status, Json(value)).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
async fn channel(State(state): State<Arc<Memory>>) -> Response {
    let gate = state.channel_gate.lock().unwrap().clone();
    if let Some((entered, held)) = gate {
        entered.notify_one();
        held.notified().await;
    }
    Json(json!({"version":1,"blob":STANDARD.encode(state.channel.lock().unwrap().as_slice())}))
        .into_response()
}
#[derive(Deserialize)]
struct PageQuery {
    owner: Option<String>,
    after: Option<String>,
    limit: Option<usize>,
}
async fn list(State(state): State<Arc<Memory>>, Query(query): Query<PageQuery>) -> Response {
    let now = state.now.load(Ordering::SeqCst) / 1000;
    let rows = state.rows.lock().unwrap();
    let items: Vec<_> = rows
        .iter()
        .filter(|(id, row)| {
            row.expiry.is_some_and(|expiry| expiry > now)
                && query.after.as_ref().is_none_or(|after| *id > after)
                && query.owner.as_ref().is_none_or(|owner| &row.owner == owner)
        })
        .take(query.limit.unwrap_or(32))
        .map(|(id, row)| json!({"record_id":id,"owner":row.owner,"version":row.version}))
        .collect();
    Json(json!({"items":items})).into_response()
}
async fn load(State(state): State<Arc<Memory>>, Path(id): Path<String>) -> Response {
    let rows = state.rows.lock().unwrap();
    match rows.get(&id).filter(|r| {
        r.expiry
            .is_none_or(|e| e > state.now.load(Ordering::SeqCst) / 1000)
    }) {
        Some(row) => {
            Json(json!({"version":row.version,"blob":STANDARD.encode(&row.blob)})).into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
async fn save(
    State(state): State<Arc<Memory>>,
    Path(id): Path<String>,
    Json(input): Json<Value>,
) -> Response {
    let gate = state.next_write_gate.lock().unwrap().take();
    if let Some((entered, held)) = gate {
        entered.notify_one();
        held.notified().await;
    }
    let fault = {
        let mut slot = state.fault.lock().unwrap();
        if slot.as_ref().is_some_and(|(target, _)| target == &id) {
            slot.take()
        } else {
            None
        }
    };
    if fault.as_ref().is_some_and(|(_, after)| !*after) {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    let Ok(blob) = STANDARD.decode(input["blob"].as_str().unwrap_or_default()) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let expiry = input["expires_at"].as_i64();
    let now = state.now.load(Ordering::SeqCst) / 1000;
    if blob.len() > 65536 || expiry.is_some_and(|e| e <= now || e > now + 7 * 86400) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let mut rows = state.rows.lock().unwrap();
    let current = rows.get(&id);
    let owner = input["owner"].as_str().unwrap_or_default();
    if current.map(|r| r.version) != input["expected_version"].as_u64()
        || current.is_some_and(|r| r.owner != owner || r.expiry != expiry)
    {
        return (
            StatusCode::CONFLICT,
            Json(json!({"actual":current.map(|r|r.version)})),
        )
            .into_response();
    }
    let version = current.map_or(1, |r| r.version + 1);
    rows.insert(
        id.clone(),
        Row {
            blob,
            version,
            owner: owner.into(),
            expiry,
        },
    );
    drop(rows);
    let hook = state.on_saved.lock().unwrap().clone();
    if let Some(hook) = hook {
        hook(&id);
    }
    if fault.is_some() {
        StatusCode::SERVICE_UNAVAILABLE.into_response()
    } else {
        Json(json!({"version":version})).into_response()
    }
}

async fn reaction_user(
    State(state): State<Arc<Memory>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if headers.get("authorization").and_then(|v| v.to_str().ok()) != Some(&format!("Bot {TOKEN}")) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    match state.reaction_users.lock().unwrap().get(&id).cloned() {
        Some(user) => Json(user).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
