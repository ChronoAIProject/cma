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
    extract::{Path, Query, State},
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

#[derive(Clone)]
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
    lose_next_put: AtomicBool,
    lose_next_put_for: Mutex<Option<String>>,
    pause_next_mutation: AtomicBool,
    pause_next_mutation_for: Mutex<Option<String>>,
    mutation_started: tokio::sync::Notify,
    resume_mutation: tokio::sync::Notify,
}
pub(super) struct Fixture {
    base: String,
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
            .route("/api/v1/gateway/connection-records", get(list))
            .route(
                "/api/v1/gateway/connection-records/{id}",
                put(save).get(load).delete(delete),
            )
            .route("/api/v1/gateway/channel-records/{id}", get(load_channel))
            .with_state(peer.clone());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            base,
            peer,
            keys: keys(19),
            server,
        }
    }
    pub(super) fn records(&self) -> RecordClient {
        RecordClient::new(&self.base, SERVICE_TOKEN).unwrap()
    }
    pub(super) fn store(&self) -> Store {
        Store::new(self.records(), self.keys.clone())
    }
    pub(super) fn service(&self) -> ConnectionService {
        let providers = Arc::new(
            ProviderClient::new(
                ProviderRegistry::parse("[]").unwrap(),
                Duration::from_secs(1),
                Duration::from_secs(1),
                2,
            )
            .unwrap(),
        );
        let management = ManagementState::new(
            providers.clone(),
            Arc::new(self.records()),
            Arc::new(CreatorAuthenticator::new(&self.base).unwrap()),
            ManagementConfig {
                public_url: "https://gateway.example".into(),
                public_api_url: "https://gateway.example".into(),
                advertise_public_api_url: true,
                master_keys: self.keys.clone(),
                cma_base_url: self.base.clone(),
                cma_service_token: Secret::new(SERVICE_TOKEN),
                nyxid_base_url: self.base.clone(),
                tools_nyxid_slug: "cmaeg".into(),
            },
            Default::default(),
        );
        let (delivery, _receiver) = tokio::sync::mpsc::channel(1);
        let tools = ToolsState {
            discord_thread_history: None,
            telegram_topics: None,
            discord_threads: None,
            native_lark_conversation_history: None,
            discord_conversation_history: None,
            complete_replies_enabled: false,
            resolver: management.resolver.clone(),
            keys: self.keys.clone(),
            providers,
            delivery: Arc::new(crate::delivery::router::ShardedScheduler::new(delivery)),
            routed_replies: None,
            tracker: TurnTracker::new(Readiness::initialized()),
            public_origin: management.public_url.clone(),
            public_api_url: management.public_url.clone(),
            advertise_public_api_url: true,
            whatsapp_templates_enabled: false,
            discord_interactions_enabled: false,
            native_discord_media_controls_enabled: false,
            native_discord_media_card_edits_enabled: false,
            native_discord_media_card_replacements_enabled: false,
            native_discord_media_card_sets_enabled: false,
            whatsapp_interactions_enabled: false,
            native_whatsapp_interactions_enabled: false,
            native_whatsapp_media_controls_enabled: false,
            consumer_interactions_enabled: false,
            telegram_effects_issuance_enabled: false,
            native_telegram_media_controls_enabled: false,
            native_telegram_media_control_edits_enabled: false,
            native_media_edits: None,
            native_whatsapp_forms: None,
            native_typing: None,
            native_lark_effects: None,
            lark_effects_issuance_enabled: false,
            lark_effects: None,
            telegram_effects: None,
            native_media_enabled: false,
            native_lark_outbound_media_enabled: false,
            native_lark_media_controls_enabled: false,
            native_lark_multi_image_controls_enabled: false,
            native_lark_media_card_replacements_enabled: false,
            native_media_presentations_enabled: false,
            native_telegram_albums_enabled: false,
            native_lark_thread_start_enabled: false,
            native_whatsapp_outbound_media_enabled: false,
            whatsapp_reactions: None,
            native_whatsapp_templates_enabled: false,
            native_whatsapp_groups_enabled: false,
            native_discord_edits_enabled: false,
            discord_effects_issuance_enabled: false,
            discord_effects: None,
            relay_text_edits_enabled: false,
            lark_cards_enabled: false,
            media: None,
            downloads: Default::default(),
        };
        ConnectionService::new(management, tools)
    }
    pub(super) fn retain_channel(
        &self,
        definition: &cmaeg_core::channel::ChannelDefinition,
    ) -> cmaeg_core::ids::ChannelId {
        let pointer = cmaeg_core::seal::ChannelPointer {
            v: 1,
            record_id: definition.record_id.clone(),
            record_url: self
                .records()
                .record_url(definition.record_id.as_str())
                .unwrap()
                .to_string(),
        };
        self.peer.channels.lock().unwrap().insert(
            definition.record_id.to_string(),
            Retained {
                owner: definition.owner.subject.clone(),
                version: 1,
                blob: STANDARD.encode(
                    seal_blob(&self.keys, definition.to_blob_json().to_string().as_bytes())
                        .unwrap(),
                ),
            },
        );
        pointer.seal_id(&self.keys).unwrap()
    }
    pub(super) fn forget_channel(&self, record_id: &str) {
        self.peer.channels.lock().unwrap().remove(record_id);
    }
}

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
pub(super) fn connection(subject: &str, organization: bool) -> Connection {
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
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if !authenticated(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    before_mutation(&peer, &id).await;
    peer.requests.lock().unwrap().push(body.clone());
    let mut rows = peer.rows.lock().unwrap();
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
    Query(query): Query<BTreeMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    if !authenticated(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    before_mutation(&peer, &id).await;
    let mut rows = peer.rows.lock().unwrap();
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

#[tokio::test]
async fn credentials_are_encrypted_in_backend_and_absent_from_public_views() {
    let fixture = Fixture::new().await;
    let record = connection("alice", false);
    fixture
        .store()
        .save_connection(&record, None)
        .await
        .unwrap();
    let ciphertext = STANDARD
        .decode(&fixture.peer.rows.lock().unwrap()[&record.id].blob)
        .unwrap();
    let requests = serde_json::to_string(&*fixture.peer.requests.lock().unwrap()).unwrap();
    let service = fixture.service();
    let view = service
        .get(&owner("alice", false), &record.id)
        .await
        .unwrap();
    let public = serde_json::to_string(&view).unwrap();
    let debug = format!("{view:?}");
    for secret in [
        CHILD_TOKEN,
        WEBHOOK_SECRET,
        REPLY_TOKEN,
        &record.setup_nonce,
        record.browser_nonce.as_deref().unwrap(),
    ] {
        assert!(!ciphertext
            .windows(secret.len())
            .any(|part| part == secret.as_bytes()));
        assert!(!requests.contains(secret));
        assert!(!public.contains(secret));
        assert!(!debug.contains(secret));
    }
    assert!(view
        .setup_url
        .as_deref()
        .unwrap()
        .starts_with("https://gateway.example/connect/start/cmaeg1.cs."));
    assert!(public.contains("Paper assistant"));
    assert!(!public.contains("origin"));
    assert!(!public.contains("material"));
    let (restored, version) = fixture.store().connection(&record.id).await.unwrap();
    assert_eq!(version, 1);
    assert_eq!(restored.material.unwrap().token, CHILD_TOKEN);
}

#[tokio::test]
async fn ciphertext_cannot_be_moved_to_another_record_or_namespace() {
    let fixture = Fixture::new().await;
    let record = connection("alice", false);
    fixture
        .store()
        .save_connection(&record, None)
        .await
        .unwrap();
    let original = fixture.peer.rows.lock().unwrap()[&record.id].clone();
    let ciphertext = STANDARD.decode(&original.blob).unwrap();
    let target = uuid::Uuid::new_v4().to_string();
    fixture
        .peer
        .rows
        .lock()
        .unwrap()
        .insert(target.clone(), original.clone());
    assert!(matches!(
        fixture.store().connection(&target).await,
        Err(Error::Unavailable)
    ));
    assert!(open_connection(&keys(20), &record.id, &ciphertext).is_err());
    assert!(open_blob(&fixture.keys, &ciphertext).is_err());
    assert!(open_interaction(&fixture.keys, &record.id, &ciphertext).is_err());
    let serialized = serde_json::to_vec(&Stored::Connection(Box::new(record.clone()))).unwrap();
    for wrong_namespace in [
        seal_blob(&fixture.keys, &serialized).unwrap(),
        seal_interaction(&fixture.keys, &record.id, &serialized).unwrap(),
    ] {
        fixture
            .peer
            .rows
            .lock()
            .unwrap()
            .get_mut(&record.id)
            .unwrap()
            .blob = STANDARD.encode(wrong_namespace);
        assert!(matches!(
            fixture.store().connection(&record.id).await,
            Err(Error::Unavailable)
        ));
    }
    let mut damaged = ciphertext;
    let middle = damaged.len() / 2;
    damaged[middle] ^= 1;
    fixture
        .peer
        .rows
        .lock()
        .unwrap()
        .get_mut(&record.id)
        .unwrap()
        .blob = STANDARD.encode(damaged);
    assert!(matches!(
        fixture.store().connection(&record.id).await,
        Err(Error::Unavailable)
    ));
}

#[test]
fn deterministic_reservations_separate_domains_and_tuple_boundaries() {
    let cases: &[(&str, &[&str])] = &[
        ("pending", &["manager-a", "actor-a"]),
        ("pending", &["manager-a", "actor-b"]),
        ("pending", &["manager-b", "actor-a"]),
        ("platform-bot", &["manager-a", "actor-a"]),
        ("pending", &["ab", "c"]),
        ("pending", &["a", "bc"]),
        ("pending", &["a", "", "bc"]),
        ("pending", &["a", "bc", ""]),
        ("pending", &["a:b", "c"]),
        ("pending", &["a", "b:c"]),
    ];
    let mut observed = BTreeSet::new();
    for (domain, parts) in cases {
        let id = index_id(domain, parts);
        assert_eq!(id, index_id(domain, parts));
        assert_eq!(uuid::Uuid::parse_str(&id).unwrap().get_version_num(), 4);
        assert!(observed.insert(id));
    }
}

#[tokio::test]
async fn simultaneous_create_and_cas_updates_have_one_durable_winner() {
    let fixture = Fixture::new().await;
    let first = fixture.store();
    let second = fixture.store();
    let mut left = connection("alice", false);
    let mut right = left.clone();
    left.suggested_name = Some("Left".into());
    right.suggested_name = Some("Right".into());
    let (a, b) = tokio::join!(
        first.save_connection(&left, None),
        second.save_connection(&right, None)
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    assert!(matches!(a, Err(Error::Conflict)) || matches!(b, Err(Error::Conflict)));
    let (mut left, version) = first.connection(&left.id).await.unwrap();
    let (mut right, other_version) = second.connection(&left.id).await.unwrap();
    assert_eq!(version, other_version);
    left.status = Status::Cancelled;
    left.browser_nonce = None;
    right.status = Status::AwaitingPlatform;
    let (a, b) = tokio::join!(
        first.save_connection(&left, Some(version)),
        second.save_connection(&right, Some(version))
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    let winner = if a.is_ok() { left } else { right };
    drop(first);
    drop(second);
    let (restored, next_version) = fixture.store().connection(&winner.id).await.unwrap();
    assert_eq!(next_version, version + 1);
    assert_eq!(restored.status, winner.status);
    assert_eq!(restored.browser_nonce, winner.browser_nonce);
    assert_eq!(restored.material.unwrap().token, CHILD_TOKEN);
}

#[tokio::test]
async fn committed_write_with_lost_response_is_recoverable_after_reopen() {
    let fixture = Fixture::new().await;
    let record = connection("alice", false);
    fixture.peer.lose_next_put.store(true, Ordering::SeqCst);
    assert!(matches!(
        fixture.store().save_connection(&record, None).await,
        Err(Error::Unavailable)
    ));
    assert!(matches!(
        fixture.store().save_connection(&record, None).await,
        Err(Error::Conflict)
    ));
    let (stored, version) = fixture.store().connection(&record.id).await.unwrap();
    assert_eq!(version, 1);
    assert_eq!(stored.fingerprint, record.fingerprint);
    assert_eq!(stored.material.unwrap().token, CHILD_TOKEN);
    assert_eq!(fixture.peer.rows.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn owner_and_organization_are_both_required_for_read_list_and_cancel() {
    let fixture = Fixture::new().await;
    let service = fixture.service();
    let mut personal = connection("alice", false);
    personal.status = Status::Ready;
    let mut organization = connection("alice", true);
    organization.status = Status::Ready;
    let mut other = connection("bob", false);
    other.status = Status::Ready;
    let draft = connection("alice", false);
    for record in [&personal, &organization, &other, &draft] {
        fixture.store().save_connection(record, None).await.unwrap();
    }
    for (caller, id) in [
        (owner("bob", false), &personal.id),
        (owner("alice", true), &personal.id),
        (owner("alice", false), &organization.id),
    ] {
        assert!(matches!(
            service.get(&caller, id).await,
            Err(Error::NotFound)
        ));
        assert!(matches!(
            service.cancel(&caller, id).await,
            Err(Error::NotFound)
        ));
    }
    let (listed, _) = service.list(&owner("alice", false), None).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].connection_ref, personal.id);
    let (listed, _) = service.list(&owner("alice", true), None).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].connection_ref, organization.id);
    let (still_ready, _) = fixture.store().connection(&personal.id).await.unwrap();
    assert_eq!(still_ready.status, Status::Ready);
}

#[tokio::test]
async fn platform_bot_claim_is_exclusive_but_exact_operation_recovery_is_idempotent() {
    let fixture = Fixture::new().await;
    let first = fixture.store();
    let second = fixture.store();
    let alice = uuid::Uuid::new_v4().to_string();
    let bob = uuid::Uuid::new_v4().to_string();
    let (a, b) = tokio::join!(
        first.claim_bot("9901", "alice", &alice),
        second.claim_bot("9901", "bob", &bob)
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    let (subject, id) = if a.is_ok() {
        ("alice", alice)
    } else {
        ("bob", bob)
    };
    fixture
        .store()
        .claim_bot("9901", subject, &id)
        .await
        .unwrap();
    assert!(matches!(
        fixture
            .store()
            .claim_bot("9901", subject, &uuid::Uuid::new_v4().to_string())
            .await,
        Err(Error::Conflict)
    ));
    assert!(matches!(
        fixture.store().claim_bot("9901", "other-owner", &id).await,
        Err(Error::Conflict)
    ));
    let claim_id = index_id("platform-bot", &["telegram", "9901"]);
    assert_eq!(fixture.peer.rows.lock().unwrap()[&claim_id].version, 1);
}

#[tokio::test]
async fn setup_capabilities_reject_wrong_purpose_expiry_tampering_and_other_keyring() {
    let fixture = Fixture::new().await;
    let service = fixture.service();
    let mut record = connection("alice", false);
    let token = service
        .capability(&record, "start", &record.setup_nonce)
        .unwrap();
    let opened = service.open_capability(&token, "start").unwrap();
    assert_eq!(opened.record_id, record.id);
    assert_eq!(opened.nonce, record.setup_nonce);
    assert!(matches!(
        service.open_capability(&token, "browser"),
        Err(Error::NotFound)
    ));
    let mut other_service = fixture.service();
    other_service.management.master_keys = keys(23);
    assert!(matches!(
        other_service.open_capability(&token, "start"),
        Err(Error::NotFound)
    ));
    let mut tampered = token.into_bytes();
    let index = tampered.len() / 2;
    tampered[index] = if tampered[index] == b'A' { b'B' } else { b'A' };
    assert!(matches!(
        service.open_capability(std::str::from_utf8(&tampered).unwrap(), "start"),
        Err(Error::NotFound)
    ));
    record.expires_at = now() - 1;
    let expired = service
        .capability(&record, "start", &record.setup_nonce)
        .unwrap();
    assert!(matches!(
        service.open_capability(&expired, "start"),
        Err(Error::NotFound)
    ));
    assert_eq!(service.view(&record).unwrap().status, Status::Expired);
    assert!(service.view(&record).unwrap().setup_url.is_none());
}

#[tokio::test]
async fn cancelling_revokes_browser_nonce_and_setup_link_but_cannot_unlink_active_channel() {
    let fixture = Fixture::new().await;
    let service = fixture.service();
    let record = connection("alice", false);
    fixture
        .store()
        .save_connection(&record, None)
        .await
        .unwrap();
    service
        .cancel(&owner("alice", false), &record.id)
        .await
        .unwrap();
    assert_eq!(fixture.store().connection(&record.id).await.unwrap().1, 2);
    service
        .cancel(&owner("alice", false), &record.id)
        .await
        .unwrap();
    let (cancelled, version) = fixture.store().connection(&record.id).await.unwrap();
    assert!(version >= 2);
    assert_eq!(cancelled.status, Status::Cancelled);
    assert!(cancelled.browser_nonce.is_none());
    assert!(service
        .get(&owner("alice", false), &record.id)
        .await
        .unwrap()
        .setup_url
        .is_none());
    let mut linked = connection("alice", false);
    linked.status = Status::Ready;
    linked.channel_record = Some(uuid::Uuid::new_v4().to_string());
    retain_bound_channel(&fixture, &linked, Some(&linked.id));
    fixture
        .store()
        .save_connection(&linked, None)
        .await
        .unwrap();
    assert!(matches!(
        service.cancel(&owner("alice", false), &linked.id).await,
        Err(Error::Conflict)
    ));
    let (unchanged, version) = fixture.store().connection(&linked.id).await.unwrap();
    assert_eq!(version, 1);
    assert_eq!(unchanged.status, Status::Ready);
    assert_eq!(unchanged.channel_record, linked.channel_record);
}

fn browser_headers(token: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        axum::http::header::COOKIE,
        format!("unrelated=ignored; __Host-cmaeg-connection={token}")
            .parse()
            .unwrap(),
    );
    headers
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

#[tokio::test]
async fn cancellation_recovers_missing_or_unrelated_channel_reservations_and_releases_exact_claim()
{
    for unrelated in [false, true] {
        let fixture = Fixture::new().await;
        let service = fixture.service();
        let mut record = connection("alice", false);
        record.status = Status::Ready;
        record.channel_record = Some(uuid::Uuid::new_v4().to_string());
        if unrelated {
            retain_bound_channel(&fixture, &record, Some("a-different-connection"));
        }
        fixture
            .store()
            .save_connection(&record, None)
            .await
            .unwrap();
        fixture
            .store()
            .claim_bot("9901", "alice", &record.id)
            .await
            .unwrap();
        assert!(!service.channel_exists(&record).await.unwrap());
        service
            .cancel(&owner("alice", false), &record.id)
            .await
            .unwrap();
        let (cancelled, _) = fixture.store().connection(&record.id).await.unwrap();
        assert_eq!(cancelled.status, Status::Cancelled);
        assert!(cancelled.channel_record.is_none());
        assert!(cancelled.channel_id.is_none());
        assert!(cancelled.browser_nonce.is_none());
        let replacement = uuid::Uuid::new_v4().to_string();
        fixture
            .store()
            .claim_bot("9901", "alice", &replacement)
            .await
            .unwrap();
        // A retry of the previous cancellation must not release a newer claim.
        service
            .cancel(&owner("alice", false), &record.id)
            .await
            .unwrap();
        assert!(matches!(
            fixture
                .store()
                .claim_bot("9901", "bob", &uuid::Uuid::new_v4().to_string())
                .await,
            Err(Error::Conflict)
        ));
        let (claim, _) = fixture
            .store()
            .load(&index_id("platform-bot", &["telegram", "9901"]))
            .await
            .unwrap();
        assert!(
            matches!(claim,Stored::BotClaim(value) if value.connection_id==replacement && value.owner=="alice")
        );
    }
}

#[tokio::test]
async fn lost_claim_release_response_can_be_retried_without_releasing_new_owner_claim() {
    let fixture = Fixture::new().await;
    let service = fixture.service();
    let record = connection("alice", false);
    fixture
        .store()
        .save_connection(&record, None)
        .await
        .unwrap();
    fixture
        .store()
        .claim_bot("9901", "alice", &record.id)
        .await
        .unwrap();
    *fixture.peer.lose_next_put_for.lock().unwrap() =
        Some(index_id("platform-bot", &["telegram", "9901"]));
    assert!(matches!(
        service.cancel(&owner("alice", false), &record.id).await,
        Err(Error::Unavailable)
    ));
    assert_eq!(
        fixture
            .store()
            .connection(&record.id)
            .await
            .unwrap()
            .0
            .status,
        Status::Cancelled
    );
    let next = uuid::Uuid::new_v4().to_string();
    fixture
        .store()
        .claim_bot("9901", "bob", &next)
        .await
        .unwrap();
    fixture
        .service()
        .cancel(&owner("alice", false), &record.id)
        .await
        .unwrap();
    fixture
        .store()
        .claim_bot("9901", "bob", &next)
        .await
        .unwrap();
    assert!(matches!(
        fixture.store().claim_bot("9901", "alice", &record.id).await,
        Err(Error::Conflict)
    ));
}

#[tokio::test]
async fn delayed_claim_release_cannot_erase_a_reacquired_claim() {
    let fixture = Fixture::new().await;
    let first = uuid::Uuid::new_v4().to_string();
    let next = uuid::Uuid::new_v4().to_string();
    fixture
        .store()
        .claim_bot("9901", "alice", &first)
        .await
        .unwrap();
    fixture
        .peer
        .pause_next_mutation
        .store(true, Ordering::SeqCst);
    let delayed_store = fixture.store();
    let delayed_id = first.clone();
    let delayed = tokio::spawn(async move {
        delayed_store
            .release_bot_claim("9901", "alice", &delayed_id)
            .await
    });
    tokio::time::timeout(
        Duration::from_secs(3),
        fixture.peer.mutation_started.notified(),
    )
    .await
    .unwrap();
    // A second replica releases Alice's same claim; Bob then acquires the slot.
    fixture
        .store()
        .release_bot_claim("9901", "alice", &first)
        .await
        .unwrap();
    fixture
        .store()
        .claim_bot("9901", "bob", &next)
        .await
        .unwrap();
    fixture.peer.resume_mutation.notify_one();
    let _ = tokio::time::timeout(Duration::from_secs(3), delayed)
        .await
        .unwrap()
        .unwrap();
    fixture
        .store()
        .claim_bot("9901", "bob", &next)
        .await
        .unwrap();
    assert!(matches!(
        fixture.store().claim_bot("9901", "alice", &first).await,
        Err(Error::Conflict)
    ));
    let (stored, version) = fixture
        .store()
        .load(&index_id("platform-bot", &["telegram", "9901"]))
        .await
        .unwrap();
    assert!(
        version >= 3,
        "releasing and reclaiming must preserve monotonic revisions"
    );
    assert!(
        matches!(stored,Stored::BotClaim(claim) if claim.owner=="bob" && claim.connection_id==next)
    );
}

#[tokio::test]
async fn cancellation_between_material_save_and_claim_releases_the_late_claim() {
    let fixture = Fixture::new().await;
    let mut record = connection("alice", false);
    record.material = None;
    fixture
        .store()
        .save_connection(&record, None)
        .await
        .unwrap();
    let claim_id = index_id("platform-bot", &["telegram", "9901"]);
    *fixture.peer.pause_next_mutation_for.lock().unwrap() = Some(claim_id.clone());
    let completing = fixture.service();
    let input = record.clone();
    let completion = tokio::spawn(async move {
        completing
            .complete(
                input,
                1,
                crate::telegram::BotIdentity {
                    id: "9901".into(),
                    username: Some("paper_fixture_bot".into()),
                    first_name: "Paper assistant".into(),
                    can_manage_bots: false,
                },
                Secret::new(CHILD_TOKEN),
            )
            .await
    });
    tokio::time::timeout(
        Duration::from_secs(3),
        fixture.peer.mutation_started.notified(),
    )
    .await
    .unwrap();
    let (persisted, version) = fixture.store().connection(&record.id).await.unwrap();
    assert_eq!(version, 2);
    assert_eq!(persisted.material.as_ref().unwrap().token, CHILD_TOKEN);
    assert!(matches!(
        fixture.store().load(&claim_id).await,
        Err(Error::NotFound)
    ));
    fixture
        .service()
        .cancel(&owner("alice", false), &record.id)
        .await
        .unwrap();
    fixture.peer.resume_mutation.notify_one();
    let result = tokio::time::timeout(Duration::from_secs(3), completion)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(result, Err(Error::Conflict)));
    let (cancelled, _) = fixture.store().connection(&record.id).await.unwrap();
    assert_eq!(cancelled.status, Status::Cancelled);
    assert!(cancelled.browser_nonce.is_none());
    let (claim, _) = fixture.store().load(&claim_id).await.unwrap();
    assert!(matches!(claim,Stored::BotClaim(value) if value.released));
    fixture
        .store()
        .claim_bot("9901", "bob", &uuid::Uuid::new_v4().to_string())
        .await
        .unwrap();
}

#[tokio::test]
async fn terminal_claim_cleanup_retries_after_lost_response_without_releasing_replacement_claim() {
    for terminal in [Status::Cancelled, Status::Expired] {
        let fixture = Fixture::new().await;
        let mut record = connection("alice", false);
        record.status = if terminal == Status::Expired {
            Status::AwaitingPlatform
        } else {
            terminal
        };
        if terminal == Status::Expired {
            record.expires_at = now() - 1;
        }
        fixture
            .store()
            .save_connection(&record, None)
            .await
            .unwrap();
        fixture
            .store()
            .claim_bot("9901", "alice", &record.id)
            .await
            .unwrap();
        let claim_id = index_id("platform-bot", &["telegram", "9901"]);
        *fixture.peer.lose_next_put_for.lock().unwrap() = Some(claim_id.clone());
        // The real worker must reconcile terminal records even though they
        // never qualify for a completion notification or platform refresh.
        let launcher = Arc::new(RecordingLauncher::default());
        let worker = fixture.service().start_completion_worker(launcher.clone());
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let (stored, _) = fixture.store().load(&claim_id).await.unwrap();
                if matches!(stored,Stored::BotClaim(value) if value.released) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        drop(worker);
        assert!(fixture.peer.lose_next_put_for.lock().unwrap().is_none());
        assert!(launcher.requests.lock().unwrap().is_empty());
        let replacement = uuid::Uuid::new_v4().to_string();
        fixture
            .store()
            .claim_bot("9901", "bob", &replacement)
            .await
            .unwrap();
        // Restart with only the durable record, as the worker's next scan does.
        let restarted = fixture.service();
        let (retained, _) = restarted.store.connection(&record.id).await.unwrap();
        restarted.reconcile_terminal_claim(&retained).await.unwrap();
        restarted.reconcile_terminal_claim(&retained).await.unwrap();
        fixture
            .store()
            .claim_bot("9901", "bob", &replacement)
            .await
            .unwrap();
        let (claim, _) = fixture.store().load(&claim_id).await.unwrap();
        assert!(
            matches!(claim,Stored::BotClaim(value) if !value.released && value.owner=="bob" && value.connection_id==replacement)
        );
    }
}

#[tokio::test]
async fn background_recovery_does_not_release_a_reservation_before_activation_was_persisted() {
    let fixture = Fixture::new().await;
    let mut record = connection("alice", false);
    record.status = Status::Ready;
    record.channel_record = Some(uuid::Uuid::new_v4().to_string());
    assert!(record.channel_id.is_none());
    fixture
        .store()
        .save_connection(&record, None)
        .await
        .unwrap();
    fixture
        .service()
        .recover_deleted_channel(record.clone(), 1)
        .await
        .unwrap();
    let (retained, version) = fixture.store().connection(&record.id).await.unwrap();
    assert_eq!(version, 1);
    assert_eq!(retained.channel_record, record.channel_record);
    assert_eq!(retained.status, Status::Ready);
}

#[tokio::test]
async fn browser_session_reopens_from_durable_record_and_rejects_duplicate_cookies() {
    use crate::http::connection_browser::browser_record;
    let fixture = Fixture::new().await;
    let record = connection("alice", false);
    fixture
        .store()
        .save_connection(&record, None)
        .await
        .unwrap();
    let token = fixture
        .service()
        .capability(&record, "browser", record.browser_nonce.as_deref().unwrap())
        .unwrap();
    let (reopened, version) = browser_record(&fixture.service(), &browser_headers(&token))
        .await
        .unwrap();
    assert_eq!(reopened.id, record.id);
    assert_eq!(version, 1);
    let mut duplicate_headers = browser_headers(&token);
    duplicate_headers.append(
        axum::http::header::COOKIE,
        format!("__Host-cmaeg-connection={token}").parse().unwrap(),
    );
    let mut duplicate_fields = HeaderMap::new();
    duplicate_fields.insert(
        axum::http::header::COOKIE,
        format!("__Host-cmaeg-connection={token}; __Host-cmaeg-connection={token}")
            .parse()
            .unwrap(),
    );
    for headers in [HeaderMap::new(), duplicate_headers, duplicate_fields] {
        assert!(matches!(
            browser_record(&fixture.service(), &headers).await,
            Err(Error::NotFound)
        ));
    }
}

#[tokio::test]
async fn browser_session_rejects_wrong_nonce_purpose_and_missing_record() {
    use crate::http::connection_browser::browser_record;
    let fixture = Fixture::new().await;
    let service = fixture.service();
    let record = connection("alice", false);
    fixture
        .store()
        .save_connection(&record, None)
        .await
        .unwrap();
    for (purpose, nonce) in [
        ("start", record.setup_nonce.as_str()),
        ("csrf", record.browser_nonce.as_deref().unwrap()),
        ("browser", "another-browser-private-nonce"),
    ] {
        let token = service.capability(&record, purpose, nonce).unwrap();
        assert!(matches!(
            browser_record(&service, &browser_headers(&token)).await,
            Err(Error::NotFound)
        ));
    }
    let missing = connection("alice", false);
    let token = service
        .capability(
            &missing,
            "browser",
            missing.browser_nonce.as_deref().unwrap(),
        )
        .unwrap();
    assert!(matches!(
        browser_record(&service, &browser_headers(&token)).await,
        Err(Error::NotFound)
    ));
}

#[tokio::test]
async fn cancelled_expired_or_replaced_durable_browser_grants_cannot_be_replayed() {
    use crate::http::connection_browser::browser_record;
    let fixture = Fixture::new().await;
    let service = fixture.service();
    for change in [
        "cancelled",
        "expired",
        "missing_nonce",
        "rotated_nonce",
        "changed_expiry",
        "rotated_link_nonce",
    ] {
        let mut record = connection("alice", false);
        fixture
            .store()
            .save_connection(&record, None)
            .await
            .unwrap();
        let token = service
            .capability(&record, "browser", record.browser_nonce.as_deref().unwrap())
            .unwrap();
        match change {
            "cancelled" => record.status = Status::Cancelled,
            "expired" => record.expires_at = now() - 1,
            "missing_nonce" => record.browser_nonce = None,
            "rotated_nonce" => {
                record.browser_nonce = Some("replacement-browser-private-nonce".into())
            }
            "rotated_link_nonce" => record.setup_nonce = "replacement-link-private-nonce".into(),
            _ => record.expires_at -= 30,
        }
        fixture
            .store()
            .save_connection(&record, Some(1))
            .await
            .unwrap();
        assert!(
            matches!(
                browser_record(&fixture.service(), &browser_headers(&token)).await,
                Err(Error::NotFound)
            ),
            "durable change {change} did not revoke prior browser authority"
        );
    }
}

#[tokio::test]
async fn setup_start_and_browser_capabilities_cannot_cross_operation_boundaries() {
    use crate::http::connection_browser::browser_record;
    let fixture = Fixture::new().await;
    let service = fixture.service();
    let first = connection("alice", false);
    let mut second = connection("bob", true);
    second.browser_nonce = Some("second-owner-browser-nonce".into());
    for record in [&first, &second] {
        fixture.store().save_connection(record, None).await.unwrap();
    }
    // Even a correctly authenticated capability for a different operation's
    // nonce cannot be moved onto this record or interpreted as a start grant.
    let mixed = service
        .capability(&second, "browser", first.browser_nonce.as_deref().unwrap())
        .unwrap();
    assert!(matches!(
        browser_record(&service, &browser_headers(&mixed)).await,
        Err(Error::NotFound)
    ));
    let valid = service
        .capability(&second, "browser", second.browser_nonce.as_deref().unwrap())
        .unwrap();
    let (restored, _) = browser_record(&service, &browser_headers(&valid))
        .await
        .unwrap();
    assert_eq!(restored.owner, "bob");
    assert!(restored.organization);
    assert!(matches!(
        service.open_capability(&valid, "start"),
        Err(Error::NotFound)
    ));
}

#[tokio::test]
async fn provisional_browser_reads_do_not_claim_a_link_and_first_cas_claim_excludes_other_browsers()
{
    use crate::http::connection_browser::browser_record;
    let fixture = Fixture::new().await;
    let service = fixture.service();
    let mut record = connection("alice", false);
    record.browser_nonce = None;
    record.browser_claimed = false;
    fixture
        .store()
        .save_connection(&record, None)
        .await
        .unwrap();
    let preview_token = service
        .capability(&record, "browser", "preview-browser-nonce")
        .unwrap();
    let human_token = service
        .capability(&record, "browser", "human-browser-nonce")
        .unwrap();
    let (preview, preview_version) = browser_record(&service, &browser_headers(&preview_token))
        .await
        .unwrap();
    let (mut human, human_version) = browser_record(&service, &browser_headers(&human_token))
        .await
        .unwrap();
    assert_eq!(preview_version, human_version);
    assert_ne!(preview.browser_nonce, human.browser_nonce);
    let (unchanged, version) = fixture.store().connection(&record.id).await.unwrap();
    assert_eq!(version, 1);
    assert!(!unchanged.browser_claimed);
    assert!(unchanged.browser_nonce.is_none());
    // This is the same CAS performed only after the browser POST's Origin and
    // CSRF checks. Reading or rendering a preview never reserves the nonce.
    human.browser_claimed = true;
    fixture
        .store()
        .save_connection(&human, Some(human_version))
        .await
        .unwrap();
    assert!(matches!(
        browser_record(&fixture.service(), &browser_headers(&preview_token)).await,
        Err(Error::NotFound)
    ));
    let (restored, _) = browser_record(&fixture.service(), &browser_headers(&human_token))
        .await
        .unwrap();
    assert!(restored.browser_claimed);
    assert_eq!(
        restored.browser_nonce.as_deref(),
        Some("human-browser-nonce")
    );
}

#[tokio::test]
async fn browser_http_exchange_preserves_link_preview_and_requires_origin_and_csrf_before_claim() {
    for advertise in [false, true] {
        browser_http_exchange("", "https://gateway.example", advertise).await;
        browser_http_exchange("/event-gateway", "https://bot.example", advertise).await;
    }
}

async fn browser_http_exchange(prefix: &str, origin: &str, advertise: bool) {
    use axum::http::header;
    let fixture = Fixture::new().await;
    let mut service = fixture.service();
    service.management.public_api_url = "https://bot.example/event-gateway".into();
    service.management.advertise_public_api_url = advertise;
    let mut record = connection("alice", false);
    record.browser_nonce = None;
    record.browser_claimed = false;
    fixture
        .store()
        .save_connection(&record, None)
        .await
        .unwrap();
    let start_token = service
        .capability(&record, "start", &record.setup_nonce)
        .unwrap();
    let app = crate::http::connection_browser::router(service.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    struct Server(tokio::task::JoinHandle<()>);
    impl Drop for Server {
        fn drop(&mut self) {
            self.0.abort();
        }
    }
    let _server = Server(tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    }));
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let mut cookies = Vec::new();
    for _ in 0..2 {
        let response = client
            .get(format!("{base}{prefix}/connect/start/{start_token}"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            response.headers()[header::LOCATION],
            format!("{prefix}/connect")
        );
        assert_eq!(response.headers()[header::REFERRER_POLICY], "no-referrer");
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let cookie = response.headers()[header::SET_COOKIE].to_str().unwrap();
        for attribute in ["Path=/", "Secure", "HttpOnly", "SameSite=Strict"] {
            assert!(cookie.contains(attribute));
        }
        cookies.push(cookie.split(';').next().unwrap().to_owned());
    }
    assert_ne!(cookies[0], cookies[1]);
    let (unchanged, version) = fixture.store().connection(&record.id).await.unwrap();
    assert_eq!(version, 1);
    assert!(!unchanged.browser_claimed);
    assert!(unchanged.browser_nonce.is_none());
    let human_token = cookies[1].split_once('=').unwrap().1;
    let human_grant = service.open_capability(human_token, "browser").unwrap();
    let csrf = service
        .capability(&record, "csrf", &human_grant.nonce)
        .unwrap();
    let page = client
        .get(format!("{base}{prefix}/connect"))
        .header(header::COOKIE, &cookies[1])
        .send()
        .await
        .unwrap();
    assert_eq!(page.status(), StatusCode::OK);
    assert!(page
        .text()
        .await
        .unwrap()
        .contains(&format!("action=\"{prefix}/connect/submit\"")));
    let target = format!("{base}{prefix}/connect/submit");
    // Invalid intent stops before any platform method while exercising real
    // HTTP extraction, Origin, CSRF and durable claim order.
    let other_origin = if prefix.is_empty() {
        "https://bot.example"
    } else {
        "https://gateway.example"
    };
    for origin in [
        None,
        Some("null"),
        Some("https://foreign.example"),
        Some(other_origin),
    ] {
        let mut request = client.post(&target).header(header::COOKIE, &cookies[1]);
        if let Some(origin) = origin {
            request = request.header(header::ORIGIN, origin);
        }
        let response = request
            .form(&[
                ("csrf_token", csrf.as_str()),
                ("intent", "fixture_invalid_intent"),
            ])
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::GONE);
    }
    let response = client
        .post(&target)
        .header(header::COOKIE, &cookies[1])
        .header(header::ORIGIN, origin)
        .header(header::ORIGIN, origin)
        .form(&[
            ("csrf_token", csrf.as_str()),
            ("intent", "fixture_invalid_intent"),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::GONE);
    let response = client
        .post(&target)
        .header(header::COOKIE, &cookies[1])
        .header(header::ORIGIN, origin)
        .form(&[
            ("csrf_token", start_token.as_str()),
            ("intent", "fixture_invalid_intent"),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::GONE);
    let (still_unclaimed, version) = fixture.store().connection(&record.id).await.unwrap();
    assert_eq!(version, 1);
    assert!(!still_unclaimed.browser_claimed);
    assert!(still_unclaimed.browser_nonce.is_none());
    let response = client
        .post(&target)
        .header(header::COOKIE, &cookies[1])
        .header(header::ORIGIN, origin)
        .form(&[
            ("csrf_token", csrf.as_str()),
            ("intent", "fixture_invalid_intent"),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let (claimed, version) = fixture.store().connection(&record.id).await.unwrap();
    assert_eq!(version, 2);
    assert!(claimed.browser_claimed);
    assert_eq!(
        claimed.browser_nonce.as_deref(),
        Some(human_grant.nonce.as_str())
    );
    let response = client
        .get(format!("{base}{prefix}/connect"))
        .header(header::COOKIE, &cookies[0])
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::GONE);
    let response = client
        .get(format!("{base}{prefix}/connect/start/{start_token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::GONE);
}

fn with_origin_channel(fixture: &Fixture, mut record: Connection) -> Connection {
    use cmaeg_core::{channel::ChannelDefinition, seal::ChannelPointer};
    let mut raw: Value = serde_json::from_str(include_str!(
        "../../../cmaeg-core/tests/fixtures/channel.json"
    ))
    .unwrap();
    raw["record_id"] = uuid::Uuid::new_v4().to_string().into();
    raw["sources"] = json!([{"type":"nyxid_relay","issuer":"https://identity.example","jwks_url":"https://identity.example/jwks",
        "key_id":"fixture-key","route_ids":["fixture-route"],"platform":"telegram","admission":{"type":"all"}}]);
    raw["partition"]["key"] = "conversation_and_sender".into();
    let mut definition: ChannelDefinition = serde_json::from_value(raw).unwrap();
    definition.profile_digest = definition.profile.digest();
    let pointer = ChannelPointer {
        v: 1,
        record_id: definition.record_id.clone(),
        record_url: fixture
            .records()
            .record_url(definition.record_id.as_str())
            .unwrap()
            .to_string(),
    };
    record.origin.event.channel_id = pointer.seal_id(&fixture.keys).unwrap();
    record.origin.event.record_id = definition.record_id.clone();
    record.origin.binding_id = definition.provider.binding_id.to_string();
    record.origin.agent_key_hash = definition.secrets.agent_key_hash.clone();
    record.origin.event.partition_key = format!(
        "{}\u{1f}{}",
        record.origin.conversation.id, record.origin.actor
    );
    record.origin.source = SourceKind::NyxidRelay {
        platform: Platform::Telegram,
        route_id: "fixture-route".into(),
    };
    fixture.peer.channels.lock().unwrap().insert(
        definition.record_id.to_string(),
        Retained {
            owner: definition.owner.subject.clone(),
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
    record
}

#[derive(Default)]
struct RecordingLauncher {
    requests: Mutex<Vec<crate::ingress::launch::LaunchRequest>>,
    observed: tokio::sync::Notify,
}
impl crate::ingress::launch::TurnLauncher for RecordingLauncher {
    fn launch(
        &self,
        request: crate::ingress::launch::LaunchRequest,
    ) -> futures_util::future::BoxFuture<'_, crate::ingress::launch::LaunchOutcome> {
        Box::pin(async move {
            use crate::ingress::launch::LaunchOutcome;
            let mut requests = self.requests.lock().unwrap();
            requests.push(request);
            let result = match requests.len() {
                1 => LaunchOutcome::Deferred,
                2 => LaunchOutcome::Refused(cmaeg_core::problem::Problem::new(
                    cmaeg_core::problem::ProblemCode::ProviderUnavailable,
                    503,
                    "fixture provider refusal",
                )),
                _ => LaunchOutcome::Accepted {
                    response_id: "fixture-accepted-turn".into(),
                },
            };
            self.observed.notify_one();
            result
        })
    }
}

#[tokio::test]
async fn completion_outbox_replays_stable_system_event_after_restart_and_only_acks_acceptance() {
    use cmaeg_core::activity::{ActivityKind, ActorKind};
    let fixture = Fixture::new().await;
    let mut record = with_origin_channel(&fixture, connection("alice", false));
    record.status = Status::Ready;
    fixture
        .store()
        .save_connection(&record, None)
        .await
        .unwrap();
    let launcher = Arc::new(RecordingLauncher::default());
    let mut reference = None;
    for attempt in 1..=3 {
        let worker = fixture.service().start_completion_worker(launcher.clone());
        tokio::time::timeout(Duration::from_secs(3), launcher.observed.notified())
            .await
            .unwrap();
        if attempt == 3 {
            tokio::time::timeout(Duration::from_secs(3), async {
                loop {
                    if fixture
                        .store()
                        .connection(&record.id)
                        .await
                        .unwrap()
                        .0
                        .notified
                    {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap();
        }
        drop(worker);
        let (stored, _) = fixture.store().connection(&record.id).await.unwrap();
        assert_eq!(stored.notified, attempt == 3);
        let durable = stored.completion_ref.unwrap();
        match &reference {
            Some(previous) => assert_eq!(&durable, previous),
            None => reference = Some(durable),
        }
    }
    let requests = launcher.requests.lock().unwrap();
    assert_eq!(requests.len(), 3);
    let first = &requests[0];
    assert_eq!(first.activity.actor.kind, ActorKind::System);
    assert_eq!(first.activity.actor.id, record.origin.actor);
    assert_eq!(first.activity.conversation, record.origin.conversation);
    assert_eq!(first.conversation_id, record.origin.event.conversation_id);
    let ActivityKind::Event { ce, .. } = &first.activity.kind else {
        panic!("completion must be a system event");
    };
    assert_eq!(ce.r#type, "gateway.connection.ready");
    assert_eq!(
        ce.data,
        Some(json!({"connection_ref":record.id,"status":"ready"}))
    );
    let serialized = serde_json::to_string(&first.activity).unwrap();
    for secret in [
        CHILD_TOKEN,
        WEBHOOK_SECRET,
        REPLY_TOKEN,
        &record.setup_nonce,
    ] {
        assert!(!serialized.contains(secret));
    }
    for request in &requests[1..] {
        assert_eq!(request.activity, first.activity);
        assert_eq!(request.event_ref, first.event_ref);
        assert_eq!(request.idempotency_key, first.idempotency_key);
        assert_eq!(request.conversation_id, first.conversation_id);
    }
    assert_eq!(Some(first.event_ref.as_str()), reference.as_deref());
}

#[tokio::test]
async fn completion_cannot_cross_origin_binding_agent_key_partition_or_connection() {
    let fixture = Fixture::new().await;
    let service = fixture.service();
    let mut record = with_origin_channel(&fixture, connection("alice", false));
    record.status = Status::Ready;
    let request = service.completion_request(&record).await.unwrap();
    record.completion_ref = Some(request.event_ref.as_str().into());
    let launcher = RecordingLauncher::default();
    for field in ["binding", "key", "partition", "connection"] {
        let mut other = record.clone();
        match field {
            "binding" => other.origin.binding_id = "another-provider-binding".into(),
            "key" => other.origin.agent_key_hash = "another-provider-key".into(),
            "partition" => other.origin.event.partition_key = "another-user-partition".into(),
            _ => other.id = uuid::Uuid::new_v4().to_string(),
        }
        assert!(
            service.notify_completion(&other, &launcher).await.is_err(),
            "changed {field} was accepted"
        );
    }
    assert!(launcher.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn external_native_bot_capability_is_omitted_until_writes_enabled() {
    let fixture = Fixture::new().await;
    for enabled in [false, true] {
        let mut service = fixture.service();
        service.management.native_bot_mentions_writes_enabled = enabled;
        let response = crate::http::connections::capabilities(
            State(service),
            Creator {
                subject: "alice".into(),
                token_hash: "capability-test".into(),
                organization: false,
            },
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        if enabled {
            assert_eq!(value["source_features"], json!(["native_bot_mentions_v1"]));
        } else {
            assert!(value.get("source_features").is_none());
        }
    }
}

#[tokio::test]
async fn public_discovery_uses_canonical_namespace_and_console_redirect_keeps_fixed_authority() {
    public_discovery(false).await;
    public_discovery(true).await;
}

async fn public_discovery(advertise: bool) {
    use axum::http::header;
    use utoipa::OpenApi;
    let fixture = Fixture::new().await;
    let mut service = fixture.service();
    service.management.public_api_url = "https://bot.example/event-gateway".into();
    service.management.advertise_public_api_url = advertise;
    let advertised = if advertise {
        "https://bot.example/event-gateway"
    } else {
        "https://gateway.example"
    };
    let record = connection("alice", false);
    assert!(service
        .view(&record)
        .unwrap()
        .setup_url
        .unwrap()
        .starts_with(&format!("{advertised}/connect/start/")));
    let app = crate::router_with_state(service.management);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let document: Value = client
        .get(format!("{base}/openapi.json"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        document["servers"],
        json!([{"url":format!("{advertised}/v1")}])
    );
    let static_document = serde_json::to_value(crate::ApiDoc::openapi()).unwrap();
    assert_eq!(document["paths"], static_document["paths"]);
    assert_eq!(document["components"], static_document["components"]);
    let docs = client.get(format!("{base}/docs")).send().await.unwrap();
    assert!(docs.headers()[header::CONTENT_SECURITY_POLICY]
        .to_str()
        .unwrap()
        .contains(if advertise {
            "img-src 'self' https://bot.example"
        } else {
            "img-src 'self' https://gateway.example"
        }));
    let html = docs.text().await.unwrap();
    for path in [
        "/docs/style.css",
        "/docs/download",
        "/openapi.json",
        "/docs/diagrams/",
    ] {
        assert!(html.contains(&format!("{advertised}{path}")), "{path}");
    }
    assert!(!html.contains("href=\"/docs"));
    if !advertise {
        assert!(!html.contains("https://bot.chrono-ai.fun/event-gateway"));
        assert!(html.contains("href=\"https://gateway.example/console\""));
    }
    let protocol_destination = format!("{advertised}/docs#protocol");
    for (path, destination) in [
        ("/docs/protocol", protocol_destination.as_str()),
        (
            "/event-gateway/legacy-console",
            "https://gateway.example/console",
        ),
        (
            "/event-gateway/legacy-console/channels/record-123?return_to=https://foreign.example",
            "https://gateway.example/console/channels/record-123",
        ),
    ] {
        let response = client.get(format!("{base}{path}")).send().await.unwrap();
        assert!(response.status().is_redirection());
        assert_eq!(response.headers()[header::LOCATION], destination);
    }
    for path in [
        "/event-gateway/legacy-console/channels/evil%2Fpath",
        "/event-gateway/legacy-console/channels/evil%5Cpath",
    ] {
        assert_eq!(
            client
                .get(format!("{base}{path}"))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND
        );
    }
    assert_eq!(
        client
            .post(format!("{base}/event-gateway/legacy-console"))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::METHOD_NOT_ALLOWED
    );
    server.abort();
}
