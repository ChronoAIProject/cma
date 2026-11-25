//! Connection lifecycle contracts against a disposable HTTP CAS store.
use super::{
    model::*,
    store::{index_id, Store},
    ConnectionService, Error,
};
use crate::{
    auth::{Creator, CreatorAuthenticator},
    channels::management::ManagementState,
    http::readiness::Readiness,
    providers::{ProviderClient, ProviderRegistry},
    records::RecordClient,
    tools::ToolsState,
    turns::TurnTracker,
    ManagementConfig,
};
use axum::{
    extract::{OriginalUri, Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, put},
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use cmaeg_core::{
    activity::{ActivityConversation, ConversationKind, SourceKind},
    channel::{Platform, Secret},
    seal::{
        open_blob, open_connection, open_interaction, seal_blob, seal_interaction, EventRefPayload,
        MasterKeyRing, ReplyAddress,
    },
};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

const CHILD_TOKEN: &str = "9901:fixture-private-child-token";
const WEBHOOK_SECRET: &str = "fixture-private-webhook-secret";
const REPLY_TOKEN: &str = "fixture-private-original-reply-token";
const SERVICE_TOKEN: &str = "fixture-record-service-token";

#[derive(Clone, PartialEq, Eq)]
struct Retained {
    owner: String,
    blob: String,
    version: u64,
}
#[derive(Default)]
struct Peer {
    rows: Mutex<BTreeMap<String, Retained>>,
    channels: Mutex<BTreeMap<String, Retained>>,
    requests: Mutex<Vec<Value>>,
    read_fault: Mutex<Option<store_diagnostics::ReadFault>>,
    telegram_calls: std::sync::atomic::AtomicUsize,
    auth_calls: std::sync::atomic::AtomicUsize,
    lose_next_put: AtomicBool,
    lose_next_put_for: Mutex<Option<String>>,
    pause_next_mutation: AtomicBool,
    pause_next_mutation_for: Mutex<Option<String>>,
    mutation_started: tokio::sync::Notify,
    resume_mutation: tokio::sync::Notify,
}
pub(super) struct Fixture {
    base: String,
    records_override: Option<RecordClient>,
    peer: Arc<Peer>,
    pub(super) keys: Arc<MasterKeyRing>,
    server: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}
impl Fixture {
    pub(super) async fn new() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let peer = Arc::new(Peer::default());
        let app = Router::new()
            .route("/api/v1/users/me", get(store_diagnostics::creator))
            .route("/api/v1/gateway/connection-records", get(list))
            .route(
                "/api/v1/gateway/connection-records/{id}",
                put(save).get(load).delete(delete),
            )
            .route(
                "/api/v1/gateway/channel-records/{id}",
                put(save).get(load_channel).delete(delete),
            )
            .route(
                "/oauth/token",
                axum::routing::post(store_diagnostics::deny_auth),
            )
            .fallback(store_diagnostics::telegram)
            .with_state(peer.clone());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            base,
            records_override: None,
            peer,
            keys: keys(19),
            server,
        }
    }
    pub(super) async fn native(mongo: &crate::replicas::mongo_fixture::Fixture) -> Self {
        let mut fixture = Self::new().await;
        fixture.records_override = Some(mongo.records(&fixture.base).await);
        fixture
    }
    pub(super) fn records(&self) -> RecordClient {
        self.records_override
            .clone()
            .unwrap_or_else(|| RecordClient::new(&self.base, SERVICE_TOKEN).unwrap())
    }
    pub(super) fn lose_put_ack_for(&self, id: &str) {
        *self.peer.lose_next_put_for.lock().unwrap() = Some(id.to_owned());
    }
    pub(super) fn store(&self) -> Store {
        Store::new(self.records(), self.keys.clone())
    }
}

#[path = "tests/fixture_service.rs"]
mod fixture_service;

fn keys(byte: u8) -> Arc<MasterKeyRing> {
    Arc::new(MasterKeyRing::parse(&format!("1:{}", STANDARD.encode([byte; 32]))).unwrap())
}
pub(super) fn owner(subject: &str, organization: bool) -> Creator {
    Creator {
        subject: subject.into(),
        token_hash: "fixture-authenticated-digest".into(),
        organization,
    }
}
pub(crate) fn connection(subject: &str, organization: bool) -> Connection {
    let timestamp = now();
    Connection {
        id: uuid::Uuid::new_v4().to_string(),
        owner: subject.into(),
        organization,
        created_at: timestamp,
        expires_at: timestamp + SETUP_TTL,
        fingerprint: "fixture-plan-digest".into(),
        origin: Origin {
            event: EventRefPayload {
                telegram_admission: None,
                v: 1,
                activity_sha256: Some("fixture-activity-digest".into()),
                channel_id: "cmaeg1.ch.fixture".parse().unwrap(),
                record_id: uuid::Uuid::new_v4().to_string().parse().unwrap(),
                event_id: "fixture-event".parse().unwrap(),
                partition_key: "private-actor".into(),
                conversation_id: "private-conversation".parse().unwrap(),
                expires_at: timestamp + SETUP_TTL,
                reply: ReplyAddress::NyxidRelay {
                    thread_reply: false,
                    message_id: "fixture-message".into(),
                    route_id: "fixture-route".into(),
                    platform: Platform::Telegram,
                    reply_token: Secret::new(REPLY_TOKEN),
                    reply_token_expires_at: timestamp + SETUP_TTL,
                },
                consumer: None,
                lineage: None,
            },
            actor: "fixture-human".into(),
            conversation: ActivityConversation {
                id: "fixture-chat".into(),
                kind: ConversationKind::Private,
                thread_id: None,
            },
            source: SourceKind::Custom,
            binding_id: "fixture-binding".into(),
            agent_key_hash: "fixture-agent-digest".into(),
        },
        suggested_name: Some("Paper assistant".into()),
        status: Status::Draft,
        setup_nonce: "fixture-private-start-nonce".into(),
        browser_nonce: Some("fixture-private-browser-nonce".into()),
        browser_claimed: true,
        webhook_retiring: false,
        webhook_lease: None,
        requested_manager: false,
        material: Some(Material {
            bot_id: "9901".into(),
            username: "paper_fixture_bot".into(),
            name: "Paper assistant".into(),
            token: CHILD_TOKEN.into(),
            webhook_secret: WEBHOOK_SECRET.into(),
            can_manage_bots: false,
        }),
        channel_record: None,
        channel_id: None,
        notified: false,
        completion_ref: None,
    }
}

fn authenticated(headers: &HeaderMap) -> bool {
    headers
        .get("authorization")
        .is_some_and(|value| value == format!("Bearer {SERVICE_TOKEN}").as_str())
}
async fn save(
    State(peer): State<Arc<Peer>>,
    Path(id): Path<String>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if !authenticated(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    before_mutation(&peer, &id).await;
    peer.requests.lock().unwrap().push(body.clone());
    let mut rows = if uri.path().starts_with("/api/v1/gateway/channel-records/") {
        peer.channels.lock().unwrap()
    } else {
        peer.rows.lock().unwrap()
    };
    let previous = rows.get(&id).map(|row| row.version);
    if previous != body["expected_version"].as_u64() {
        return (
            StatusCode::CONFLICT,
            Json(json!({"actual_version":previous})),
        )
            .into_response();
    }
    let version = previous.unwrap_or(0) + 1;
    let subject = body["owner"].as_str().unwrap().to_owned();
    let targeted_loss = {
        let mut target = peer.lose_next_put_for.lock().unwrap();
        if target.as_deref() == Some(&id) {
            target.take();
            true
        } else {
            false
        }
    };
    rows.insert(
        id,
        Retained {
            owner: subject,
            blob: body["blob"].as_str().unwrap().to_owned(),
            version,
        },
    );
    if peer.lose_next_put.swap(false, Ordering::SeqCst) || targeted_loss {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    Json(json!({"version":version})).into_response()
}
async fn load(
    State(peer): State<Arc<Peer>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if !authenticated(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if let Some(response) = store_diagnostics::read_fault(&peer, "get").await {
        return response;
    }
    match peer.rows.lock().unwrap().get(&id) {
        Some(row) => Json(json!({"blob":row.blob,"version":row.version})).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
async fn load_channel(
    State(peer): State<Arc<Peer>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if !authenticated(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    match peer.channels.lock().unwrap().get(&id) {
        Some(row) => Json(json!({"blob":row.blob,"version":row.version})).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
async fn delete(
    State(peer): State<Arc<Peer>>,
    Path(id): Path<String>,
    OriginalUri(uri): OriginalUri,
    Query(query): Query<BTreeMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    if !authenticated(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    before_mutation(&peer, &id).await;
    let mut rows = if uri.path().starts_with("/api/v1/gateway/channel-records/") {
        peer.channels.lock().unwrap()
    } else {
        peer.rows.lock().unwrap()
    };
    let Some(current) = rows.get(&id) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if query
        .get("expected_version")
        .and_then(|value| value.parse::<u64>().ok())
        != Some(current.version)
    {
        return (
            StatusCode::CONFLICT,
            Json(json!({"actual":current.version})),
        )
            .into_response();
    }
    rows.remove(&id);
    StatusCode::NO_CONTENT.into_response()
}
async fn before_mutation(peer: &Peer, id: &str) {
    let targeted_pause = {
        let mut target = peer.pause_next_mutation_for.lock().unwrap();
        if target.as_deref() == Some(id) {
            target.take();
            true
        } else {
            false
        }
    };
    if peer.pause_next_mutation.swap(false, Ordering::SeqCst) || targeted_pause {
        peer.mutation_started.notify_one();
        tokio::time::timeout(Duration::from_secs(5), peer.resume_mutation.notified())
            .await
            .unwrap();
    }
}
async fn list(
    State(peer): State<Arc<Peer>>,
    Query(query): Query<BTreeMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    if !authenticated(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if let Some(response) = store_diagnostics::read_fault(&peer, "page").await {
        return response;
    }
    let rows = peer.rows.lock().unwrap();
    let items = rows
        .iter()
        .filter(|(id, row)| {
            query.get("owner").is_none_or(|owner| owner == &row.owner)
                && query.get("after").is_none_or(|after| *id > after)
        })
        .take(
            query
                .get("limit")
                .and_then(|value| value.parse().ok())
                .unwrap_or(100),
        )
        .map(|(id, row)| json!({"record_id":id,"owner":row.owner,"version":row.version}))
        .collect::<Vec<_>>();
    Json(json!({"items":items})).into_response()
}

fn retain_bound_channel(fixture: &Fixture, record: &Connection, connection_ref: Option<&str>) {
    let mut raw: Value = serde_json::from_str(include_str!(
        "../../../cmaeg-core/tests/fixtures/channel.json"
    ))
    .unwrap();
    let id = record.channel_record.as_deref().unwrap();
    let identity = json!({"kind":if record.organization {"organization"} else {"human"},"subject":record.owner});
    raw["record_id"] = id.into();
    raw["owner"] = identity.clone();
    raw["profile"]["metadata"]["owner"] = identity;
    raw["connection_ref"] = json!(connection_ref);
    let mut definition: cmaeg_core::channel::ChannelDefinition =
        serde_json::from_value(raw).unwrap();
    definition.profile_digest = definition.profile.digest();
    fixture.peer.channels.lock().unwrap().insert(
        id.to_owned(),
        Retained {
            owner: record.owner.clone(),
            version: 1,
            blob: STANDARD.encode(
                seal_blob(
                    &fixture.keys,
                    definition.to_blob_json().to_string().as_bytes(),
                )
                .unwrap(),
            ),
        },
    );
}

mod browser;
mod cancellation;
mod discovery;
mod lifecycle;
mod outbox;

mod store_diagnostics;

#[path = "tests/setup_gate.rs"]
mod setup_gate;
