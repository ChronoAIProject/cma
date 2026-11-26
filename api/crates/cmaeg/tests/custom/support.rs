use super::{
    fake_provider::{FakeProvider, FakeProviderConfig},
    receiver::Receiver,
    relay_support::Fixture,
};
use cmaeg::{
    auth::webhook,
    delivery::{
        custom::CustomDelivery, engine::DeliveryEngine, relay_client::RelayClient,
        sync::SyncRegistry, webhook_client::WebhookClient,
    },
    http::{self, events::CustomIngressState},
    ingress::IngressState,
    providers::{ProviderClient, ProviderRegistry},
    tools::ToolsState,
    turns::{TurnRunner, TurnTracker},
    TurnConfig,
};
use cmaeg_core::channel::Secret;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{sync::mpsc, task::JoinHandle};
pub struct App {
    pub fixture: Fixture,
    pub provider: FakeProvider,
    pub receiver: Receiver,
    pub secret: Secret,
    pub key: String,
    pub url: String,
    pub base: String,
    pub tracker: Arc<TurnTracker>,
    pub waiters: Arc<SyncRegistry>,
    engine: Option<DeliveryEngine>,
    server: JoinHandle<()>,
}
impl Drop for App {
    fn drop(&mut self) {
        self.server.abort();
    }
}
impl App {
    pub async fn new(
        edit: impl FnOnce(&mut Value),
        provider_config: FakeProviderConfig,
        turn_config: TurnConfig,
        failures: usize,
    ) -> Self {
        Self::build(edit, provider_config, turn_config, failures, false).await
    }
    /// [`App::standard`] with an edited channel, served with the inbound media
    /// switch on or off.
    pub async fn media(edit: impl FnOnce(&mut Value), media_enabled: bool) -> Self {
        Self::build(
            edit,
            Default::default(),
            Default::default(),
            0,
            media_enabled,
        )
        .await
    }
    async fn build(
        edit: impl FnOnce(&mut Value),
        provider_config: FakeProviderConfig,
        turn_config: TurnConfig,
        failures: usize,
        media_enabled: bool,
    ) -> Self {
        let receiver = Receiver::new(failures).await;
        let secret = Secret::new(uuid::Uuid::new_v4().to_string());
        let key = format!("nyx_{}", uuid::Uuid::new_v4().simple());
        let fixture = Fixture::new(|value| {
            value["sources"] = json!([{"type":"custom","auth":{"type":"hmac","key_id":"ingress"}}]);
            value["partition"]["key"] = "per_event".into();
            value["reply"]["sinks"] = json!([{"type":"sync"},{"type":"result_webhook","url":"https://receiver.example/events","hmac_key_id":"result"}]);
            value["secrets"]["hmac_secrets"] = json!([["ingress",secret.as_str()],["result",secret.as_str()]]);
            value["secrets"]["agent_key_hash"] = format!("sha256:{:x}", Sha256::digest(key.as_bytes())).into();
            value["secrets"]["provider_credential"] = key.clone().into();
            edit(value);
        }, Default::default()).await;
        let waiters = Arc::new(SyncRegistry::default());
        let tracker = TurnTracker::new(http::readiness::Readiness::initialized());
        let (delivery, commands) = mpsc::channel(256);
        let engine = DeliveryEngine::start(
            commands,
            fixture.state.master_keys.clone(),
            RelayClient::new("https://nyx.example", Duration::from_secs(5)).unwrap(),
            Default::default(),
            tracker.clone(),
            CustomDelivery {
                public_origin: "https://gateway.example".into(),
                waiters: waiters.clone(),
                webhooks: WebhookClient::with_transport(receiver.transport.clone()),
                activity: Arc::new(cmaeg::turns::observer::NoopObserver),
            },
        );
        let provider = FakeProvider::start(provider_config).await;
        let entry = |slug: &str| json!({"slug":slug,"base_url":provider.base_url(),"kind":"responses_http","paths":{"agent_card":"/.well-known/agent-card.json","binding":"/api/v1/cmaeg/bindings/{binding_id}","conversation":"/api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}","responses":"/api/v2/responses"},"max_inflight":8});
        let registry =
            ProviderRegistry::parse(&json!([entry("cma_codex"), entry("nyxbot")]).to_string())
                .unwrap();
        let providers = Arc::new(
            ProviderClient::new(registry, Duration::from_secs(1), Duration::from_secs(2), 8)
                .unwrap(),
        );
        let delivery = Arc::new(cmaeg::delivery::router::ShardedScheduler::new(delivery));
        let runner = Arc::new(TurnRunner::new(
            providers.clone(),
            tracker.clone(),
            delivery.clone(),
            turn_config,
        ));
        let ingress = IngressState::new(
            fixture.state.resolver.clone(),
            fixture.state.master_keys.clone(),
            Default::default(),
            runner,
        )
        .unwrap()
        .with_media_enabled(media_enabled);
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
            resolver: fixture.state.resolver.clone(),
            keys: fixture.state.master_keys.clone(),
            providers,
            delivery,
            routed_replies: None,
            tracker: tracker.clone(),
            public_origin: "https://gateway.example".into(),
            public_api_url: "https://gateway.example".into(),
            public_api_aliases: vec![],
            advertise_public_api_url: true,
            media: None,
            downloads: Default::default(),
        };
        let router = http::events::router(CustomIngressState {
            ingress,
            waiters: waiters.clone(),
        })
        .merge(http::tools::router(tools));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let url = format!("{base}/v1/channels/{}/events", fixture.id);
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Self {
            fixture,
            provider,
            receiver,
            secret,
            key,
            url,
            base,
            tracker,
            waiters,
            engine: Some(engine),
            server,
        }
    }
    pub async fn standard() -> Self {
        Self::new(|_| {}, Default::default(), Default::default(), 0).await
    }
    pub fn event(&self, id: &str) -> Value {
        json!({"specversion":"1.0","id":id,"type":"order.created","source":"https://orders.example","subject":"customer-1","data":{"z":1,"a":2}})
    }
    pub fn request(&self, url: &str, body: String) -> reqwest::RequestBuilder {
        self.request_media(url, body, "application/cloudevents+json")
    }
    pub fn request_media(&self, url: &str, body: String, media: &str) -> reqwest::RequestBuilder {
        let timestamp = time::OffsetDateTime::now_utc().unix_timestamp().to_string();
        let signature = webhook::signature(&self.secret, "delivery-1", &timestamp, body.as_bytes());
        self.fixture
            .client
            .post(url)
            .header("content-type", media)
            .header("webhook-id", "delivery-1")
            .header("webhook-timestamp", timestamp)
            .header("webhook-signature", signature)
            .body(body)
    }
    pub async fn drain(&mut self) {
        assert!(
            self.tracker
                .drain(Instant::now() + Duration::from_secs(10))
                .await
        );
        assert!(
            self.engine
                .take()
                .unwrap()
                .drain(Instant::now() + Duration::from_secs(30))
                .await
        );
    }
}
/// A custom-source channel whose ingress launches into the fixture's recording launcher.
///
/// Tests call the handler in process so they control the request body's owner.
pub struct Recorded {
    pub fixture: Fixture,
    pub state: CustomIngressState,
    secret: Secret,
}
impl Recorded {
    pub async fn new() -> Self {
        let secret = Secret::new(uuid::Uuid::new_v4().to_string());
        let fixture = Fixture::new(
            |value| {
                value["sources"] =
                    json!([{"type":"custom","auth":{"type":"hmac","key_id":"ingress"}}]);
                value["partition"]["key"] = "per_event".into();
                value["reply"]["sinks"] = json!([{"type":"sync"}]);
                value["secrets"]["hmac_secrets"] = json!([["ingress", secret.as_str()]]);
            },
            Default::default(),
        )
        .await;
        let state = CustomIngressState {
            ingress: fixture.state.clone(),
            waiters: Arc::new(SyncRegistry::default()),
        };
        Self {
            fixture,
            state,
            secret,
        }
    }
    /// A signed structured event request carrying `body`, whose bytes equal `signed`.
    pub fn request(&self, signed: &[u8], body: axum::body::Body) -> axum::extract::Request {
        let timestamp = time::OffsetDateTime::now_utc().unix_timestamp().to_string();
        let signature = webhook::signature(&self.secret, "delivery-1", &timestamp, signed);
        axum::http::Request::post(format!("/v1/channels/{}/events", self.fixture.id))
            .header("content-type", "application/cloudevents+json")
            .header("webhook-id", "delivery-1")
            .header("webhook-timestamp", timestamp)
            .header("webhook-signature", signature)
            .body(body)
            .unwrap()
    }
}
