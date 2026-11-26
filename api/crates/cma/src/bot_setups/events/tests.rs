//! Real gateway attestation, CMA advance router and durable stores. Only identity
//! login and downstream provisioning ports are fixtures; context is never stubbed.
use super::*;
use crate::store::{
    bot_setup_contexts::MongoBotSetupContextStore, bot_setups::MongoBotSetupStore,
    AuthEncryptionKey,
};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Extension, Json, Router,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use cmaeg::{
    channels::resolver::ChannelResolver,
    http::readiness::Readiness,
    providers::{ProviderClient, ProviderRegistry},
    records::RecordClient,
    tools::ToolsState,
    turns::TurnTracker,
    ChannelCacheConfig,
};
use cmaeg_core::{
    activity::Activity,
    channel::ChannelDefinition,
    ids::ConversationId,
    seal::{
        event_ref::activity_digest, seal_blob, ChannelPointer, EventRef, EventRefPayload,
        MasterKeyRing, ReplyAddress,
    },
};
use layer2_core::resource_access::ResourceOwner;
use mongodb::Client;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::task::JoinHandle;

const BINDING: &str = "bnd_fatherfixture";
const KEY: &str = "nyxid_ag_father_fixture_1234567890";

struct Dependencies {
    http: reqwest::Client,
    gateway: String,
    contexts: Arc<MongoBotSetupContextStore>,
}
#[async_trait::async_trait]
impl BotSetupDependencies for Dependencies {
    async fn event(
        &self,
        binding: &str,
        credential: &str,
        reference: &str,
        activity: Option<&Value>,
    ) -> Result<SetupEvent, BotError> {
        EventVerifier {
            http: &self.http,
            gateway: &self.gateway,
            binding: BINDING,
            contexts: self.contexts.as_ref(),
        }
        .verify(binding, credential, reference, activity)
        .await
    }
    async fn identity(&self, actor: &ExternalActor) -> Result<SetupIdentity, BotError> {
        assert_eq!(actor.actor_id, "user-1");
        Ok(SetupIdentity {
            login_url: Some("https://identity.example/login".into()),
            ..Default::default()
        })
    }
    async fn confirm_identity(
        &self,
        _: &ExternalActor,
        _: &str,
    ) -> Result<SetupIdentity, BotError> {
        panic!("fresh event must request login")
    }
    async fn owners(&self, _: &ExternalActor, _: &str) -> Result<Vec<SetupOwnerChoice>, BotError> {
        panic!("not linked")
    }
    async fn defaults(&self) -> Result<SetupConfiguration, BotError> {
        Ok(serde_json::from_value(json!({
            "agent":{"model":"fixture","thinking_effort":"medium","permission_level":"guarded","skills":[],"system_prompt":""},
            "workspace":{"location":{"type":"directory","name":"work"},"name":"work","setup_commands":[],"agent_profiles":[]},
            "sandbox":{"cpu_millis":1000,"memory_mib":2048,"nyxid_services":[],"agent_provider":"codex","model_access":"chronoai_managed","max_workspaces":1,"max_agents_per_workspace":1,"workspace_profiles":[],"software_install_commands":[],"environment_variables":[]},
            "availability":{"warmup_enabled":false,"warmup_count":1,"always_keep_alive":false}
        })).unwrap())
    }
    async fn validate_configuration(&self, _: &SetupConfiguration) -> Result<(), BotError> {
        Ok(())
    }
    async fn services(
        &self,
        _: &ExternalActor,
        _: &str,
        _: &ResourceOwner,
    ) -> Result<Vec<SetupServiceChoice>, BotError> {
        panic!("not linked")
    }
    async fn connect(
        &self,
        _: &ExternalActor,
        _: &str,
        _: &ResourceOwner,
        _: &str,
    ) -> Result<String, BotError> {
        panic!("not linked")
    }
    async fn external_bots(
        &self,
        _: &ExternalActor,
        _: &str,
        _: &ResourceOwner,
    ) -> Result<Vec<SetupExternalBot>, BotError> {
        panic!("not linked")
    }
    async fn connection_authority_stamp(&self, _: &BotSetup) -> Result<String, BotError> {
        unreachable!("fixture does not request channel connections")
    }
    async fn channel_connection(&self, _: &BotSetup) -> Result<SetupConnectionStatus, BotError> {
        unreachable!("fixture does not request channel connections")
    }
    async fn cancel_channel_connection(&self, _: &BotSetup) -> Result<(), BotError> {
        unreachable!("fixture does not request channel connections")
    }
    async fn provision(&self, _: &BotSetup) -> Result<SetupResult, BotError> {
        panic!("not linked")
    }
    async fn result(&self, _: &BotSetup) -> Result<SetupResult, BotError> {
        panic!("not provisioned")
    }
}

#[derive(Clone)]
struct Backend {
    blob: Arc<Mutex<Vec<u8>>>,
    contexts: Arc<Mutex<Arc<MongoBotSetupContextStore>>>,
    model_calls: Arc<AtomicUsize>,
}
async fn record(State(state): State<Backend>) -> Json<Value> {
    Json(json!({"blob":STANDARD.encode(state.blob.lock().unwrap().as_slice()),"version":1}))
}
async fn retained(
    State(state): State<Backend>,
    Path((binding, conversation, event)): Path<(String, String, String)>,
    headers: HeaderMap,
) -> Response {
    if binding != BINDING
        || headers.get("authorization").and_then(|v| v.to_str().ok())
            != Some(&format!("Bearer {KEY}"))
    {
        return StatusCode::FORBIDDEN.into_response();
    }
    let store = state.contexts.lock().unwrap().clone();
    match store.get(&binding, &conversation, &event).await.unwrap() {
        Some(snapshot) => Json(snapshot).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
async fn model(State(state): State<Backend>) -> StatusCode {
    state.model_calls.fetch_add(1, Ordering::SeqCst);
    StatusCode::INTERNAL_SERVER_ERROR
}
async fn serve(router: Router) -> (String, JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    (
        base,
        tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        }),
    )
}
fn gateway(backend: &str, keys: Arc<MasterKeyRing>) -> ToolsState {
    let records = Arc::new(RecordClient::new(backend, "service-fixture").unwrap());
    let registry = ProviderRegistry::parse(&json!([{"slug":"cma_codex","base_url":backend,"kind":"responses_http","paths":{"agent_card":"/.well-known/agent-card.json","binding":"/api/v1/cmaeg/bindings/{binding_id}","conversation":"/api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}","responses":"/api/v2/responses"},"max_inflight":1}]).to_string()).unwrap();
    ToolsState {
        commonmark_relay: Default::default(),
        native_media_presentations_enabled: false,
        native_discord_voice_enabled: false,
        native_whatsapp_forms: None,
        native_typing: None,
        discord_thread_history: None,
        telegram_topics: None,
        discord_threads: None,
        native_lark_conversation_history: None,
        discord_conversation_history: None,
        complete_replies_enabled: false,
        routed_replies: None,
        telegram_effects_issuance_enabled: false,
        native_telegram_media_controls_enabled: false,
        native_discord_media_controls_enabled: false,
        native_discord_media_card_edits_enabled: false,
        native_discord_media_card_replacements_enabled: false,
        native_discord_media_card_sets_enabled: false,
        native_telegram_media_control_edits_enabled: false,
        native_media_edits: None,
        native_lark_effects: None,
        lark_effects_issuance_enabled: false,
        lark_effects: None,
        telegram_effects: None,
        native_discord_edits_enabled: false,
        discord_effects_issuance_enabled: false,
        discord_effects: None,
        lark_cards_enabled: false,
        resolver: ChannelResolver::new(records, keys.clone(), ChannelCacheConfig::default()),
        keys,
        providers: Arc::new(
            ProviderClient::new(registry, Duration::from_secs(1), Duration::from_secs(1), 1)
                .unwrap(),
        ),
        delivery: Arc::new(cmaeg::delivery::router::ShardedScheduler::new(
            tokio::sync::mpsc::channel(1).0,
        )),
        tracker: TurnTracker::new(Readiness::initialized()),
        public_origin: "https://gateway.example".into(),
        public_api_url: "https://gateway.example".into(),
        public_api_aliases: vec![],
        advertise_public_api_url: true,
        whatsapp_templates_enabled: false,
        whatsapp_interactions_enabled: false,
        native_whatsapp_interactions_enabled: false,
        native_whatsapp_media_controls_enabled: false,
        discord_interactions_enabled: false,
        consumer_interactions_enabled: false,
        native_media_enabled: false,
        native_lark_outbound_media_enabled: false,
        native_lark_sticker_replies_enabled: false,
        native_lark_media_controls_enabled: false,
        native_lark_multi_image_controls_enabled: false,
        native_lark_media_card_replacements_enabled: false,
        native_telegram_structured_replies_enabled: false,
        native_whatsapp_structured_replies_enabled: false,
        native_telegram_albums_enabled: false,
        native_lark_thread_start_enabled: false,
        native_whatsapp_outbound_media_enabled: false,
        whatsapp_reactions: None,
        native_whatsapp_templates_enabled: false,
        native_whatsapp_groups_enabled: false,
        relay_text_edits_enabled: false,
        media: None,
        downloads: Default::default(),
    }
}
fn advance_router(
    store: Arc<MongoBotSetupStore>,
    contexts: Arc<MongoBotSetupContextStore>,
    gateway: &str,
) -> Router {
    let service = BotSetupService::new(
        store,
        Arc::new(Dependencies {
            http: reqwest::Client::new(),
            gateway: gateway.into(),
            contexts,
        }),
    );
    layer1_public::rest::bot_setup_router(service)
        .layer(Extension(layer1_public::AuthMethod::BindingKey {
            binding_id: BINDING.into(),
        }))
        .layer(Extension(layer1_public::DelegatedCredential::new(
            KEY.into(),
        )))
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable MongoDB replica set"]
async fn fresh_father_event_reaches_login_with_verified_durable_context_without_model_turn() {
    let mongo = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let database = format!("cma_father_context_test_{}", uuid::Uuid::new_v4().simple());
    let encryption = AuthEncryptionKey::from_base64(&STANDARD.encode([31; 32])).unwrap();
    let contexts = Arc::new(
        MongoBotSetupContextStore::open(&mongo, &database, "fixture", &encryption)
            .await
            .unwrap(),
    );
    let setups = Arc::new(
        MongoBotSetupStore::open(&mongo, &database, "fixture", &encryption)
            .await
            .unwrap(),
    );
    let backend = Backend {
        blob: Default::default(),
        contexts: Arc::new(Mutex::new(contexts.clone())),
        model_calls: Default::default(),
    };
    let (provider, provider_task) = serve(
        Router::new()
            .route("/api/v1/gateway/channel-records/{id}", get(record))
            .route(
                "/api/v1/cmaeg/bindings/{binding}/conversations/{conversation}/events/{event}",
                get(retained),
            )
            .route("/api/v2/responses", axum::routing::post(model))
            .with_state(backend.clone()),
    )
    .await;
    let keys = Arc::new(MasterKeyRing::parse(&format!("1:{}", STANDARD.encode([41; 32]))).unwrap());
    let mut definition: ChannelDefinition = serde_json::from_str(include_str!(
        "../../../../cmaeg-core/tests/fixtures/channel.json"
    ))
    .unwrap();
    definition.record_id = uuid::Uuid::new_v4().to_string().parse().unwrap();
    definition.provider.binding_id = BINDING.parse().unwrap();
    definition.secrets.agent_key_hash = format!("sha256:{:x}", Sha256::digest(KEY.as_bytes()));
    definition.secrets.provider_credential = Some(cmaeg_core::channel::Secret::new(KEY));
    definition.profile_digest = definition.profile.digest();
    let pointer = ChannelPointer {
        v: 1,
        record_id: definition.record_id.clone(),
        record_url: format!(
            "{provider}/api/v1/gateway/channel-records/{}",
            definition.record_id
        ),
    };
    let channel = pointer.seal_id(&keys).unwrap();
    *backend.blob.lock().unwrap() =
        seal_blob(&keys, definition.to_blob_json().to_string().as_bytes()).unwrap();
    let mut activity: Activity = serde_json::from_str(include_str!(
        "../../../../cmaeg-core/tests/fixtures/activity.json"
    ))
    .unwrap();
    if let cmaeg_core::activity::ActivityKind::Message { text, .. } = &mut activity.kind {
        *text = "x".repeat(32 * 1024);
    }
    let partition = activity.partition_key(&definition.partition);
    let conversation = ConversationId::derive(&keys, &channel, 1, &partition);
    let expiry = mongodb::bson::DateTime::now().timestamp_millis() / 1000 + 1800;
    let payload = EventRefPayload {
        telegram_admission: None,
        v: 1,
        activity_sha256: Some(activity_digest(&activity)),
        channel_id: channel.clone(),
        record_id: pointer.record_id.clone(),
        event_id: activity.event_id.clone(),
        partition_key: partition,
        conversation_id: conversation.clone(),
        expires_at: expiry,
        reply: ReplyAddress::None,
        consumer: None,
        lineage: None,
    };
    let reference = EventRef::seal(&payload, &keys).unwrap();
    assert!(reference.as_str().len() < 4096);
    let (gateway_url, gateway_task) =
        serve(cmaeg::http::tools::router(gateway(&provider, keys.clone()))).await;
    let (cma, cma_task) = serve(advance_router(setups, contexts.clone(), &gateway_url)).await;
    let http = reqwest::Client::new();
    let context_url = format!("{gateway_url}/v1/events/{}/context", reference.as_str());
    assert_eq!(
        http.get(&context_url)
            .bearer_auth(KEY)
            .send()
            .await
            .unwrap()
            .status(),
        503,
        "fresh event has no provider command context"
    );
    let body = json!({"event_ref":reference,"activity":activity});
    let response = http
        .post(format!("{cma}/api/v1/bot-setups/advance"))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let view: Value = response.json().await.unwrap();
    assert_eq!(view["step"], "login");
    assert_eq!(
        view["identity"]["login_url"],
        "https://identity.example/login"
    );
    assert_eq!(backend.model_calls.load(Ordering::SeqCst), 0);
    for (path, forged) in [
        ("actor", json!({"id":"another-user","kind":"human"})),
        (
            "kind",
            json!({"type":"message","text":"forged","mentions_bot":true}),
        ),
        ("source", json!({"type":"custom"})),
    ] {
        let mut forged_body = body.clone();
        forged_body["activity"][path] = forged;
        assert!(!http
            .post(format!("{cma}/api/v1/bot-setups/advance"))
            .json(&forged_body)
            .send()
            .await
            .unwrap()
            .status()
            .is_success());
    }
    assert_eq!(
        http.post(&context_url)
            .bearer_auth("nyxid_ag_other_fixture_1234567890")
            .json(&activity)
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    let mut expired = payload;
    expired.expires_at = 1;
    let expired = EventRef::seal(&expired, &keys).unwrap();
    assert_eq!(
        http.post(format!(
            "{gateway_url}/v1/events/{}/context",
            expired.as_str()
        ))
        .bearer_auth(KEY)
        .json(&activity)
        .send()
        .await
        .unwrap()
        .status(),
        410
    );

    // Restart both workflow storage and gateway: no provider command is admitted.
    cma_task.abort();
    gateway_task.abort();
    drop(contexts);
    let reopened = Arc::new(
        MongoBotSetupContextStore::open(&mongo, &database, "fixture", &encryption)
            .await
            .unwrap(),
    );
    *backend.contexts.lock().unwrap() = reopened.clone();
    let (gateway_url, gateway_task) =
        serve(cmaeg::http::tools::router(gateway(&provider, keys))).await;
    let response = http
        .get(format!(
            "{gateway_url}/v1/events/{}/context",
            reference.as_str()
        ))
        .bearer_auth(KEY)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let restored: Value = response.json().await.unwrap();
    assert_eq!(
        restored["activity"]["kind"]["text"].as_str().unwrap().len(),
        32 * 1024
    );
    assert_eq!(restored["activity_verified"], true);
    assert!(reopened
        .get(
            "another-binding",
            conversation.as_str(),
            activity.event_id.as_str()
        )
        .await
        .unwrap()
        .is_none());
    let setups = Arc::new(
        MongoBotSetupStore::open(&mongo, &database, "fixture", &encryption)
            .await
            .unwrap(),
    );
    let (cma, cma_task) = serve(advance_router(setups, reopened.clone(), &gateway_url)).await;
    let duplicate: Value = http
        .post(format!("{cma}/api/v1/bot-setups/advance"))
        .json(&body)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(duplicate["setup_id"], view["setup_id"]);
    assert_eq!(duplicate["revision"], view["revision"]);
    assert_eq!(backend.model_calls.load(Ordering::SeqCst), 0);
    // The agent uses the retained, signed event and a separate typed operation.
    // Multiple operations in one model turn must not consume the human event.
    let action_url = format!("{cma}/api/v1/bot-setups/actions");
    let read: Value = http
        .post(&action_url)
        .json(&json!({"event_ref":reference,"request":{
            "request_id":"agent-view", "action":{"type":"view"}
        }}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(read["setup_id"], view["setup_id"]);
    let draft = json!({"event_ref":reference,"request":{
        "request_id":"agent-name",
        "expected":{"setup_id":read["setup_id"],"revision":read["revision"]},
        "action":{"type":"draft","draft":{"name":"Paper assistant","instructions":"Explain papers."}}
    }});
    let drafted = http.post(&action_url).json(&draft).send().await.unwrap();
    assert_eq!(drafted.status(), 200);
    let drafted: Value = drafted.json().await.unwrap();
    assert_eq!(drafted["name"], "Paper assistant");
    let replayed: Value = http
        .post(&action_url)
        .json(&draft)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(replayed["revision"], drafted["revision"]);
    let mut altered = draft.clone();
    altered["request"]["action"]["draft"]["name"] = "Different name".into();
    assert_eq!(
        http.post(&action_url)
            .json(&altered)
            .send()
            .await
            .unwrap()
            .status(),
        409
    );
    let mut forged = draft;
    forged["activity"] = json!({"actor":{"id":"another-user"}});
    assert_eq!(
        http.post(&action_url)
            .json(&forged)
            .send()
            .await
            .unwrap()
            .status(),
        422
    );
    let records = mongo
        .database(&database)
        .collection::<mongodb::bson::Document>("bot_setup_contexts");
    let raw = records
        .find_one(mongodb::bson::doc! {})
        .await
        .unwrap()
        .unwrap();
    assert!(!format!("{raw:?}").contains(&"x".repeat(100)));
    assert!(!format!("{raw:?}").contains(reference.as_str()));
    gateway_task.abort();
    cma_task.abort();
    provider_task.abort();
    mongo.database(&database).drop().await.unwrap();
}

#[test]
fn verified_input_preserves_user_words_for_agent_action_consent() {
    for text in [
        "/setup",
        "/setup create",
        "create",
        "Create Bot",
        "  hi  ",
        "/start link_abc-123",
        "link_abc-123",
        "make a paper assistant",
        "{\"agent\":{}}",
    ] {
        let kind = json!({"type":"message", "text":text});
        let input = setup_input(&kind).unwrap();
        assert!(matches!(input, SetupInput::Text(ref value) if value == text));
        assert_eq!(kind["text"], text);
    }
    for (name, args, expected) in [
        ("setup", "create", "/setup create"),
        ("start", "link_abc-123", "/start link_abc-123"),
        ("create", "", "/create"),
    ] {
        let input = setup_input(&json!({"type":"command", "name":name, "args":args})).unwrap();
        assert!(matches!(input, SetupInput::Text(ref value) if value == expected));
    }
}

#[test]
fn connection_completion_is_a_notification_without_human_consent() {
    let mut activity = json!({"actor":{"kind":"system","id":"original-sender"},
        "kind":{"type":"event","ce":{"type":"gateway.connection.ready","data":{"connection_ref":"opaque","status":"ready"}}}});
    assert!(is_connection_notification(&activity));
    assert!(matches!(
        setup_input(&activity["kind"]).unwrap(),
        SetupInput::Notification
    ));
    activity["kind"]["ce"]["type"] = "arbitrary.event".into();
    assert!(!is_connection_notification(&activity));
    activity["kind"] = json!({"type":"message","text":"Create Bot"});
    assert!(!is_connection_notification(&activity));
}
