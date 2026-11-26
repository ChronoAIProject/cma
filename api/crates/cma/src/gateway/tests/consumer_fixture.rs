//! Real Gateway routes and restricted Mongo storage; only external platform I/O is fake.
use super::*;
use axum::{
    extract::{Path, State},
    routing::post,
    Json, Router,
};
use cmaeg::{
    connections::{model::*, store::Store},
    records::{mongo::MongoRecords, RecordClient},
};
use cmaeg_core::{
    activity::{ActivityConversation, ConversationKind, SourceKind},
    channel::Secret,
    seal::{EventRefPayload, MasterKeyRing, ReplyAddress},
};
use std::sync::Mutex;

pub(super) const PUBLIC: &str = "https://gateway.example";
pub(super) struct ConsumerGateway {
    pub base: String,
    pub connection: String,
    pub store: Store,
    pub platform: Arc<Mutex<Platform>>,
    _database: fixture::database::DatabaseFixture,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}
#[derive(Default)]
pub(super) struct Platform {
    pub webhook: String,
    pub calls: Vec<String>,
}
impl Drop for ConsumerGateway {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}
impl ConsumerGateway {
    pub async fn start(server: &Server) -> Self {
        let database = fixture::database::DatabaseFixture::new(&Gateway::binary()).await;
        let mongo = mongodb::Client::with_uri_str(&database.uri).await.unwrap();
        let records = Arc::new(
            RecordClient::new(&server.base, "fixture-service")
                .unwrap()
                .with_database(
                    MongoRecords::open(mongo.database(&database.name))
                        .await
                        .unwrap(),
                ),
        );
        let keys = Arc::new(
            MasterKeyRing::parse(&format!(
                "1:{}",
                base64::Engine::encode(&base64::engine::general_purpose::STANDARD, [7_u8; 32])
            ))
            .unwrap(),
        );
        let store = Store::new((*records).clone(), keys.clone());
        let connection = ready_connection();
        store.save_connection(&connection, None).await.unwrap();
        store
            .claim_bot("9901", "owner", &connection.id)
            .await
            .unwrap();
        let platform = Arc::new(Mutex::new(Platform::default()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let platform_origin = format!("http://{}", listener.local_addr().unwrap());
        let app = Router::new()
            .route("/bot9901:consumer-fixture/{method}", post(platform_call))
            .with_state(platform.clone());
        let platform_task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let config = cmaeg::Config {
            profile_revisions_enabled: false,
            github_enabled: false,
            native_media_presentations_enabled: false,
            native_discord_voice_enabled: false,
            native_split_text_refs_enabled: false,
            native_telegram_message_events_writes_enabled: false,
            native_telegram_member_joins_enabled: false,
            relay_telegram_member_joins_enabled: false,
            managed_telegram_setup_enabled: true,
            native_lark_writes_enabled: false,
        native_lark_intake_enabled: false,
        native_lark_controls_enabled: false,
        native_lark_forms_enabled: false,
        native_lark_media_enabled: false,
        native_lark_stickers_enabled: false,
        native_lark_mixed_history_enabled: false,
        native_telegram_media_edits_enabled: false,
        native_telegram_media_controls_enabled: false,
        native_telegram_media_control_edits_enabled: false,
        native_discord_media_edits_enabled: false,
        native_discord_media_controls_enabled: false,
        native_discord_media_card_edits_enabled: false,
        native_discord_media_card_replacements_enabled: false,
        native_discord_media_card_sets_enabled: false,
        native_lark_message_effects_enabled: false,
        native_lark_received_reactions_enabled: false,
        native_typing_enabled: false,
        native_whatsapp_writes_enabled: false,
            native_whatsapp_groups_writes_enabled: false,
        native_whatsapp_message_events_enabled: false,
        native_whatsapp_forms_enabled: false,
        native_whatsapp_flow_writes_enabled: false,
            native_whatsapp_media_enabled: false,
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
            native_whatsapp_interactions_enabled: false,
            native_whatsapp_media_controls_enabled: false,
            native_whatsapp_reactions_enabled: false,
            native_telegram_received_reactions_enabled: false,
            native_discord_received_reactions_enabled: false,
            native_whatsapp_templates_enabled: false,
            native_whatsapp_groups_enabled: false,
            native_discord_writes_enabled: false,
            native_discord_thread_history_enabled: false,
        native_telegram_topic_lifecycle_enabled: false,
        native_telegram_topic_creation_enabled: false,
        native_telegram_general_topic_lifecycle_enabled: false,
        native_discord_thread_lifecycle_enabled: false,
        native_lark_conversation_history_enabled: false,
        native_discord_conversation_history_enabled: false,
            native_discord_reactions_enabled: false,
            native_discord: Default::default(),
            native_bot_mentions_writes_enabled: false,
            thread_contract_writes_enabled: false,
            lark_resource_writes_enabled: false,
        lark_message_effects_writes_enabled: false,
            multi_consumer_writes_enabled: false,
            dynamic_sources_writes_enabled: false,
            relay_raw_verification_writes_enabled: false,
        outbound_messages_writes_enabled: false,
        event_side_messages_enabled: false,
            listen: "127.0.0.1:0".parse().unwrap(), metrics_listen: "127.0.0.1:0".parse().unwrap(),
            providers: cmaeg::providers::ProviderRegistry::parse(&json!([{
                "slug":"cma_codex","base_url":server.base,"kind":"responses_http","max_inflight":8,
                "paths":{"agent_card":"/.well-known/agent-card.json","binding":"/api/v1/cmaeg/bindings/{binding_id}",
                    "conversation":"/api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}","responses":"/api/v2/responses"}
            }]).to_string()).unwrap(),
            provider_connect_timeout: Duration::from_secs(2), provider_request_timeout: Duration::from_secs(5),
            provider_max_inflight: 8, cma_service_account: None,
            management: cmaeg::ManagementConfig { console_public_url: None, public_url: PUBLIC.into(), public_api_url: PUBLIC.into(), public_api_aliases: vec![], advertise_public_api_url: true, master_keys: keys.clone(),
                cma_base_url: server.base.clone(), cma_service_token: Secret::new("fixture-service"),
                nyxid_base_url: server.base.clone(), tools_nyxid_slug: "cmaeg".into() },
            media: Default::default(), stt: Default::default(), transformer: Default::default(), console: Default::default(), routing: Default::default(),
            channel_cache: Default::default(), ingress: Default::default(), turns: Default::default(), delivery: Default::default(),
        };
        let providers = Arc::new(cmaeg::providers::ProviderClient::from_config(&config).unwrap());
        let auth = Arc::new(cmaeg::auth::CreatorAuthenticator::new(&server.base).unwrap());
        let mut state = cmaeg::http::ManagementState::new(
            providers.clone(),
            records,
            auth,
            config.management,
            config.channel_cache,
        );
        state.managed_telegram_setup_enabled = config.managed_telegram_setup_enabled;
        state.telegram = Some(Arc::new(
            cmaeg::telegram::TelegramClient::new(&platform_origin, Duration::from_secs(2)).unwrap(),
        ));
        let (delivery, _receiver) = tokio::sync::mpsc::channel(1);
        let tools = cmaeg::tools::ToolsState {
            commonmark_relay: Default::default(),
            native_media_presentations_enabled: false,
            native_discord_voice_enabled: false,
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
            native_whatsapp_forms: None,
            native_typing: None,
            native_lark_effects: None,
            lark_effects_issuance_enabled: false,
            lark_effects: None,
            telegram_effects: None,
            native_discord_edits_enabled: false,
            discord_effects_issuance_enabled: false,
            discord_effects: None,
            lark_cards_enabled: false,
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
            resolver: state.resolver.clone(),
            keys,
            providers,
            delivery: Arc::new(cmaeg::delivery::router::ShardedScheduler::new(delivery)),
            tracker: cmaeg::turns::TurnTracker::new(state.readiness.clone()),
            public_origin: PUBLIC.into(),
            public_api_url: PUBLIC.into(),
            public_api_aliases: vec![],
            advertise_public_api_url: true,
            media: None,
            downloads: Default::default(),
        };
        let connections = cmaeg::connections::ConnectionService::new(state.clone(), tools);
        let app =
            cmaeg::router_with_state(state).merge(cmaeg::http::connections::router(connections));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let gateway = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self {
            base,
            connection: connection.id,
            store,
            platform,
            _database: database,
            tasks: vec![platform_task, gateway],
        }
    }
}
async fn platform_call(
    State(platform): State<Arc<Mutex<Platform>>>,
    Path(method): Path<String>,
    Json(body): Json<Value>,
) -> Json<Value> {
    let mut platform = platform.lock().unwrap();
    platform.calls.push(method.clone());
    let result = match method.as_str() {
        "getMe" => {
            json!({"id":9901,"is_bot":true,"first_name":"Research channel","username":"research_fixture_bot"})
        }
        "getWebhookInfo" => json!({"url":platform.webhook}),
        "setWebhook" => {
            platform.webhook = body["url"].as_str().unwrap().into();
            json!(true)
        }
        "deleteWebhook" => {
            platform.webhook.clear();
            json!(true)
        }
        _ => panic!("unexpected platform method"),
    };
    Json(json!({"ok":true,"result":result}))
}
fn ready_connection() -> Connection {
    let now = layer2_core::seconds_from_rfc3339(&layer2_core::now_rfc3339()).unwrap() as i64;
    Connection {
        id: uuid::Uuid::new_v4().to_string(),
        owner: "owner".into(),
        organization: false,
        created_at: now,
        expires_at: now + SETUP_TTL,
        fingerprint: "fixture-plan".into(),
        origin: Origin {
            event: EventRefPayload {
                telegram_admission: None,
                v: 1,
                consumer: None,
                lineage: None,
                activity_sha256: Some("fixture-activity".into()),
                channel_id: "cmaeg1.ch.fixture".parse().unwrap(),
                record_id: uuid::Uuid::new_v4().to_string().parse().unwrap(),
                event_id: "fixture-event".parse().unwrap(),
                partition_key: "fixture-partition".into(),
                conversation_id: "fixture-conversation".parse().unwrap(),
                expires_at: now + SETUP_TTL,
                reply: ReplyAddress::Telegram {
                    bot_id: "9901".into(),
                    chat_id: "42".into(),
                    message_id: "1".into(),
                    thread_id: None,
                },
            },
            actor: "42".into(),
            conversation: ActivityConversation {
                id: "42".into(),
                kind: ConversationKind::Private,
                thread_id: None,
            },
            source: SourceKind::Custom,
            binding_id: "fixture-origin".into(),
            agent_key_hash: "fixture-hash".into(),
        },
        suggested_name: Some("Research channel".into()),
        status: Status::Ready,
        setup_nonce: "fixture-setup".into(),
        browser_nonce: None,
        browser_claimed: true,
        requested_manager: false,
        material: Some(Material {
            bot_id: "9901".into(),
            username: "research_fixture_bot".into(),
            name: "Research channel".into(),
            token: "9901:consumer-fixture".into(),
            webhook_secret: "fixture-webhook".into(),
            can_manage_bots: false,
        }),
        channel_record: None,
        channel_id: None,
        webhook_retiring: false,
        webhook_lease: None,
        notified: false,
        completion_ref: None,
    }
}
