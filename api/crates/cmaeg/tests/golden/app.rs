//! The composed gateway the golden suites drive: management, relay, native
//! Telegram, custom ingress, the tools API and MCP over one listener, with the
//! production turn runner and delivery engine in front of fake CMA, NyxID,
//! Telegram and provider backends. Channels are created through the public
//! management API, so their records are exactly what the gateway writes.
use super::{
    fake_cma::FakeCma,
    fake_provider::{FakeProvider, ReceivedRequest},
    identity::Identity,
    relay_support::{now_rfc3339, Envelope},
};
use axum::{extract::Path, routing::post, Json, Router};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use cmaeg::{
    auth::{webhook, CreatorAuthenticator},
    channels::management::ManagementState,
    delivery::{
        custom::CustomDelivery, engine::DeliveryEngine, relay_client::RelayClient,
        router::ShardedScheduler, sync::SyncRegistry,
    },
    http::{self, events::CustomIngressState},
    ingress::IngressState,
    providers::{ProviderClient, ProviderRegistry},
    records::RecordClient,
    telegram::TelegramClient,
    tools::ToolsState,
    turns::{TurnRunner, TurnTracker},
    ChannelCacheConfig, ManagementConfig,
};
use cmaeg_core::{channel::Secret, ids::ChannelId, seal::MasterKeyRing};
use serde_json::{json, Value};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::task::JoinHandle;

const PUBLIC_URL: &str = "http://127.0.0.1:8090";
/// The relay issuer channels declare; its JWKS is relocated to the fake NyxID.
pub const ISSUER: &str = "https://nyx.example";
pub const WEBHOOK_SECRET: &str = "native-webhook-golden";

/// The agent key every golden channel is created with. It is fixed so the
/// captured authorization hashes are stable, and spelled with a low-entropy
/// pattern so it can never be mistaken for a real credential.
pub fn agent_key() -> String {
    format!("nyx_{}", "golden".repeat(4))
}

/// The HMAC secret of every golden custom source, fixed for the same reason.
pub fn hmac_secret() -> Secret {
    Secret::new("golden".repeat(6))
}

pub struct GoldenApp {
    pub provider: FakeProvider,
    pub state: ManagementState,
    pub base: String,
    pub client: reqwest::Client,
    tracker: Arc<TurnTracker>,
    engine: Option<DeliveryEngine>,
    _cma: FakeCma,
    identity: Identity,
    tasks: Vec<JoinHandle<()>>,
}

impl Drop for GoldenApp {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

impl GoldenApp {
    pub async fn start() -> Self {
        let cma = FakeCma::start(false).await;
        let identity = Identity::start().await;
        let (telegram_base, telegram_task) =
            serve(Router::new().route("/{token}/{method}", post(telegram_call))).await;
        let provider = FakeProvider::start(Default::default()).await;
        let registry = ProviderRegistry::parse(&json!([{"slug":"cma_codex","base_url":provider.base_url(),"kind":"responses_http","paths":{
            "agent_card":"/.well-known/agent-card.json","binding":"/api/v1/cmaeg/bindings/{binding_id}","conversation":"/api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}","responses":"/api/v2/responses"},"max_inflight":8}]).to_string()).unwrap();
        let providers = Arc::new(
            ProviderClient::new(registry, Duration::from_secs(1), Duration::from_secs(3), 8)
                .unwrap(),
        );
        let keys =
            Arc::new(MasterKeyRing::parse(&format!("1:{}", STANDARD.encode([1; 32]))).unwrap());
        let state = ManagementState::new(
            providers.clone(),
            Arc::new(RecordClient::new(&cma.base_url, "cma-service").unwrap()),
            Arc::new(CreatorAuthenticator::new(&identity.base).unwrap()),
            ManagementConfig {
                console_public_url: None,
                master_keys: keys.clone(),
                public_url: PUBLIC_URL.into(),
                public_api_url: PUBLIC_URL.into(),
                public_api_aliases: vec![],
                advertise_public_api_url: true,
                cma_base_url: cma.base_url.clone(),
                tools_nyxid_slug: "cmaeg".into(),
                cma_service_token: Secret::new("cma-service"),
                nyxid_base_url: identity.base.clone(),
            },
            ChannelCacheConfig::default(),
        );
        let telegram = TelegramClient::new(&telegram_base, Duration::from_secs(2)).unwrap();
        let tracker = TurnTracker::new(state.readiness.clone());
        let (delivery, outcomes) = tokio::sync::mpsc::channel(256);
        let waiters = Arc::new(SyncRegistry::default());
        let engine = DeliveryEngine::start(
            outcomes,
            keys.clone(),
            RelayClient::new(&identity.base, Duration::from_secs(5))
                .unwrap()
                .with_telegram(telegram.clone()),
            Default::default(),
            tracker.clone(),
            CustomDelivery::new(PUBLIC_URL.into(), waiters.clone()),
        );
        let delivery = Arc::new(ShardedScheduler::new(delivery));
        let runner = Arc::new(TurnRunner::new(
            providers.clone(),
            tracker.clone(),
            delivery.clone(),
            Default::default(),
        ));
        let ingress = IngressState::new(
            state.resolver.clone(),
            keys.clone(),
            Default::default(),
            runner,
        )
        .unwrap()
        .with_telegram(Arc::new(telegram));
        let tools = ToolsState {
            discord_thread_history: None,
            telegram_topics: None,
            discord_threads: None,
            native_lark_conversation_history: None,
            discord_conversation_history: None,
            complete_replies_enabled: false,
            commonmark_relay: Default::default(),
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
            native_lark_sticker_replies_enabled: false,
            native_lark_media_controls_enabled: false,
            native_lark_multi_image_controls_enabled: false,
            native_lark_media_card_replacements_enabled: false,
            native_media_presentations_enabled: false,
            native_discord_voice_enabled: false,
            native_telegram_structured_replies_enabled: false,
            native_whatsapp_structured_replies_enabled: false,
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
            resolver: state.resolver.clone(),
            keys,
            providers,
            delivery,
            routed_replies: None,
            tracker: tracker.clone(),
            public_origin: PUBLIC_URL.into(),
            public_api_url: PUBLIC_URL.into(),
            public_api_aliases: vec![],
            advertise_public_api_url: true,
            media: None,
            downloads: Default::default(),
        };
        let router = cmaeg::router_with_state(state.clone())
            .merge(http::events::router(CustomIngressState {
                ingress: ingress.clone(),
                waiters,
            }))
            .merge(http::telegram::router(ingress.clone()))
            .merge(http::callbacks::router(ingress))
            .merge(http::tools::router(tools.clone()))
            .merge(cmaeg::mcp::router(tools));
        let (base, server) = serve(router).await;
        Self {
            provider,
            state,
            base,
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .unwrap(),
            tracker,
            engine: Some(engine),
            _cma: cma,
            identity,
            tasks: vec![telegram_task, server],
        }
    }

    /// Create a channel through `POST /v1/channels` and return its id. Relay
    /// sources then have their JWKS relocated to the fake NyxID, as the public
    /// API pins HTTPS issuer discovery; nothing else in the record changes.
    pub async fn create(&self, sources: Value, partition: &str, sinks: Value) -> String {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../cmaeg-core/tests/fixtures/channel.json"
        ))
        .unwrap();
        let mut profile = fixture["profile"].clone();
        profile["metadata"]["owner"] = json!({"kind":"human","subject":"alice"});
        let body = json!({"schema_version":1,"name":"golden","profile":profile,
            "provider":{"slug":"cma_codex"},"sources":sources,
            "partition":{"version":1,"key":partition},
            "reply":{"auto_final":true,"ack_after_ms":0,"sinks":sinks},
            "credentials":{"agent_key":agent_key(),"channel_key":format!("nyx_{}", "route".repeat(4)),
                "telegram_bot_token":"77:golden","telegram_webhook_secret":WEBHOOK_SECRET,
                "hmac_secrets":[{"key_id":"ingress","secret":hmac_secret().as_str()}]}});
        let response = self
            .client
            .post(format!("{}/v1/channels", self.base))
            .bearer_auth("token-alice")
            .json(&body)
            .send()
            .await
            .unwrap();
        let status = response.status();
        let created = response.json::<Value>().await.unwrap();
        assert_eq!(status, 201, "{created}");
        let channel = created["channel_id"].as_str().unwrap().to_owned();
        self.relocate_jwks(&channel).await;
        channel
    }

    async fn relocate_jwks(&self, channel: &str) {
        let keys = &self.state.master_keys;
        let pointer = ChannelId::new(channel).unwrap().open(keys).unwrap().payload;
        let record = self.state.records.get(&pointer.record_url).await.unwrap();
        let opened = cmaeg_core::seal::open_blob(keys, &record.blob).unwrap();
        let mut definition: Value = serde_json::from_slice(&opened.payload).unwrap();
        for source in definition["sources"].as_array_mut().unwrap() {
            if source["type"] == "nyxid_relay" {
                source["jwks_url"] = format!("{}/jwks", self.identity.base).into();
            }
        }
        let blob =
            cmaeg_core::seal::seal_blob(keys, &serde_json::to_vec(&definition).unwrap()).unwrap();
        self.state
            .records
            .put(&pointer.record_id, "alice", &blob, Some(record.version))
            .await
            .unwrap();
        self.state.resolver.invalidate(channel);
    }

    /// Deliver a signed NyxID relay callback for `envelope`.
    pub async fn callback(&self, channel: &str, envelope: &Envelope) -> reqwest::Response {
        let (body, token) = envelope.signed();
        self.client
            .post(format!("{}/v1/callbacks/{channel}", self.base))
            .header("content-type", "application/json")
            .header("x-nyxid-callback-token", token)
            .header(
                "x-nyxid-platform",
                envelope.payload["platform"].as_str().unwrap(),
            )
            .header("x-nyxid-message-id", "message-1")
            .header("x-nyxid-timestamp", now_rfc3339())
            .body(body)
            .send()
            .await
            .unwrap()
    }

    /// Deliver a native Telegram webhook update.
    pub async fn telegram(&self, channel: &str, update: &Value) -> reqwest::Response {
        self.client
            .post(format!("{}/v1/telegram/{channel}", self.base))
            .header("x-telegram-bot-api-secret-token", WEBHOOK_SECRET)
            .json(update)
            .send()
            .await
            .unwrap()
    }

    /// Deliver a signed structured CloudEvent, with `query` appended to the URL.
    pub async fn event(&self, channel: &str, query: &str, event: &Value) -> reqwest::Response {
        let body = event.to_string();
        let timestamp = time::OffsetDateTime::now_utc().unix_timestamp().to_string();
        let signature =
            webhook::signature(&hmac_secret(), "delivery-1", &timestamp, body.as_bytes());
        self.client
            .post(format!("{}/v1/channels/{channel}/events{query}", self.base))
            .header("content-type", "application/cloudevents+json")
            .header("webhook-id", "delivery-1")
            .header("webhook-timestamp", timestamp)
            .header("webhook-signature", signature)
            .body(body)
            .send()
            .await
            .unwrap()
    }

    /// Wait until every launched turn has finished, so its provider calls
    /// are all captured.
    pub async fn quiet(&self) {
        tokio::time::timeout(Duration::from_secs(10), async {
            while !self.tracker.is_empty() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("turns finish");
    }

    /// The provider calls captured so far, in arrival order.
    pub fn captured(&self) -> Vec<ReceivedRequest> {
        self.provider.log()
    }

    pub async fn drain(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(10);
        assert!(self.tracker.drain(deadline).await);
        assert!(self.engine.take().unwrap().drain(deadline).await);
    }
}

/// A Telegram Bot API stand-in that confirms every send in the requested chat.
async fn telegram_call(
    Path((_token, method)): Path<(String, String)>,
    Json(body): Json<Value>,
) -> Json<Value> {
    let result = match method.as_str() {
        "sendMessage" | "editMessageText" => {
            let chat = body["chat_id"]
                .as_i64()
                .or_else(|| body["chat_id"].as_str().and_then(|id| id.parse().ok()))
                .unwrap_or_default();
            json!({"message_id":100,"chat":{"id":chat}})
        }
        _ => json!(true),
    };
    Json(json!({"ok":true,"result":result}))
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
