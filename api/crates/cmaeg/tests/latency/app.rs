use super::identity::Identity;
use super::{
    fake_cma::FakeCma,
    fake_provider::FakeProvider,
    relay_support::{now_rfc3339, Envelope},
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use cmaeg::{
    auth::CreatorAuthenticator,
    channels::management::ManagementState,
    console::{
        memory::{ConsoleMemory, Schedule, Sources},
        observer::{ConsoleObserver, Observers},
    },
    delivery::{engine::DeliveryEngine, relay_client::RelayClient},
    http,
    ingress::IngressState,
    providers::{ProviderClient, ProviderRegistry},
    records::RecordClient,
    tools::ToolsState,
    turns::{TurnRunner, TurnTracker},
    ChannelCacheConfig, ConsoleMemoryConfig, ManagementConfig,
};
use cmaeg_core::{channel::Secret, seal::MasterKeyRing};
use serde_json::{json, Value};
use std::{
    num::NonZeroU32,
    sync::{atomic::Ordering, Arc},
    time::{Duration, Instant},
};

pub const LANES: usize = 16;
/// The console's observer and memory, when the run has the console on.
pub struct ConsoleOn {
    pub observer: ConsoleObserver,
    pub memory: Arc<ConsoleMemory>,
}

pub struct App {
    pub batch_gate: Arc<super::batch_gate::BatchGate>,
    pub console: Option<ConsoleOn>,
    pub cma: FakeCma,
    pub identity: Identity,
    pub provider: FakeProvider,
    pub state: ManagementState,
    pub tracker: Arc<TurnTracker>,
    pub base: String,
    pub channel: String,
    pub client: reqwest::Client,
    pub agent_key: String,
    pub engine: Option<DeliveryEngine>,
    pub restore_delay: u64,
    server: tokio::task::JoinHandle<()>,
}
impl Drop for App {
    fn drop(&mut self) {
        self.server.abort();
    }
}
impl App {
    pub async fn new() -> Self {
        Self::build(false).await
    }
    /// The same gateway with `CMAEG_CONSOLE_ENABLED=true`: every observer
    /// composed as `serve.rs` composes it, and a warm owner index.
    pub async fn with_console() -> Self {
        Self::build(true).await
    }
    async fn build(console: bool) -> Self {
        let restore_delay = std::env::var("CMAEG_LATENCY_RESTORE_DELAY_MS")
            .map(|value| value.parse::<u64>().unwrap())
            .unwrap_or(50);
        assert!(restore_delay > 0);
        let cma = FakeCma::start(false).await;
        cma.get_delay_ms.store(restore_delay, Ordering::SeqCst);
        let identity = Identity::start().await;
        let provider = FakeProvider::start(Default::default()).await;
        let registry = ProviderRegistry::parse(&json!([{"slug":"cma_codex","base_url":provider.base_url(),"kind":"responses_http","paths":{
            "agent_card":"/.well-known/agent-card.json","binding":"/api/v1/cmaeg/bindings/{binding_id}","conversation":"/api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}","responses":"/api/v2/responses"},"max_inflight":256}]).to_string()).unwrap();
        let providers = Arc::new(
            ProviderClient::new(
                registry,
                Duration::from_secs(1),
                Duration::from_secs(3),
                256,
            )
            .unwrap(),
        );
        let keys =
            Arc::new(MasterKeyRing::parse(&format!("1:{}", STANDARD.encode([1; 32]))).unwrap());
        let mut observers = Observers::new(console);
        let state = ManagementState::observed(
            providers.clone(),
            Arc::new(RecordClient::new(&cma.base_url, "cma-service").unwrap()),
            Arc::new(CreatorAuthenticator::new(&identity.base).unwrap()),
            ManagementConfig {
                console_public_url: None,
                master_keys: keys.clone(),
                public_url: "http://127.0.0.1:8090".into(),
                public_api_url: "http://127.0.0.1:8090".into(),
                public_api_aliases: vec![],
                advertise_public_api_url: true,
                cma_base_url: cma.base_url.clone(),
                tools_nyxid_slug: "cmaeg".into(),
                cma_service_token: Secret::new("cma-service"),
                nyxid_base_url: identity.base.clone(),
            },
            ChannelCacheConfig {
                restores_per_minute: NonZeroU32::new(60_000).unwrap(),
                ..Default::default()
            },
            observers.index.clone(),
        );
        let tracker = TurnTracker::new(state.readiness.clone());
        let (delivery, outcomes) = tokio::sync::mpsc::channel(256);
        let waiters = Arc::default();
        let engine = DeliveryEngine::start_observed(
            outcomes,
            keys.clone(),
            RelayClient::new(&identity.base, Duration::from_secs(5)).unwrap(),
            Default::default(),
            tracker.clone(),
            cmaeg::delivery::custom::CustomDelivery::new("http://127.0.0.1:8090".into(), waiters),
            observers.activity.clone(),
        );
        let delivery = Arc::new(cmaeg::delivery::router::ShardedScheduler::new(delivery));
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
            observers.launcher(runner),
        )
        .unwrap()
        .with_activity_observer(observers.activity.clone());
        let console = observers.queue.take().map(|queue| {
            let memory = Arc::new(ConsoleMemory::new(
                &ConsoleMemoryConfig::default(),
                Sources {
                    heads: state.records.clone(),
                    reader: Arc::new(state.clone()),
                },
                Schedule::from_config(&ConsoleMemoryConfig::default()),
            ));
            // The drainer only: the warm index is listed below.
            memory.spawn(queue, cmaeg::http::readiness::Readiness::initialized());
            memory
        });
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
            public_origin: "http://127.0.0.1:8090".into(),
            public_api_url: "http://127.0.0.1:8090".into(),
            public_api_aliases: vec![],
            advertise_public_api_url: true,
            media: None,
            downloads: Default::default(),
        };
        let batch_gate = super::batch_gate::BatchGate::new(cma.get_gate.clone());
        let router = cmaeg::router_with_state(state.clone())
            .merge(super::batch_gate::router(ingress, batch_gate.clone()))
            .merge(http::tools::router(tools.clone()))
            .merge(cmaeg::mcp::router(tools));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap();
        let agent_key = format!("nyx_agent_{}", uuid::Uuid::new_v4().simple());
        let body = definition(&agent_key);
        let response = client
            .post(format!("{base}/v1/channels"))
            .bearer_auth("token-alice")
            .json(&body)
            .send()
            .await
            .unwrap();
        let status = response.status();
        let body = response.json::<Value>().await.unwrap();
        assert_eq!(status, 201, "{body}");
        let channel = body["channel_id"].as_str().unwrap().to_owned();
        // The public creator API pins HTTPS issuer discovery. Relocate only the
        // test record's JWKS endpoint to the local fake; signature/issuer checks
        // and the production restore path still execute unchanged.
        let pointer = cmaeg_core::ids::ChannelId::new(&channel)
            .unwrap()
            .open(&state.master_keys)
            .unwrap()
            .payload;
        let record = cma.records().remove(pointer.record_id.as_str()).unwrap();
        let opened = cmaeg_core::seal::open_blob(&state.master_keys, &record.blob).unwrap();
        let mut definition: Value = serde_json::from_slice(&opened.payload).unwrap();
        definition["sources"][0]["jwks_url"] = format!("{}/jwks", identity.base).into();
        let blob = cmaeg_core::seal::seal_blob(
            &state.master_keys,
            &serde_json::to_vec(&definition).unwrap(),
        )
        .unwrap();
        state
            .records
            .put(&pointer.record_id, "alice", &blob, Some(record.version))
            .await
            .unwrap();
        let console = match console {
            Some(memory) => {
                memory.list_all().await;
                assert_eq!(memory.index_status().records, 1, "the index is warm");
                Some(ConsoleOn {
                    observer: observers.console.take().expect("composed with the console"),
                    memory,
                })
            }
            None => None,
        };
        Self {
            batch_gate,
            console,
            cma,
            identity,
            provider,
            state,
            tracker,
            base,
            channel,
            client,
            agent_key,
            engine: Some(engine),
            server,
            restore_delay,
        }
    }
    pub async fn callback(&self, id: &str, lane: usize) -> reqwest::Response {
        self.callback_request(id, lane).send().await.unwrap()
    }
    pub fn cold_callbacks(
        &self,
        events: impl IntoIterator<Item = (String, usize)>,
    ) -> Vec<reqwest::RequestBuilder> {
        // Signing is synchronous. Finish every envelope before invalidating so
        // later callers do not sign while the first caller's restore completes.
        let events: Vec<_> = events.into_iter().collect();
        let requests = events
            .iter()
            .map(|(id, lane)| self.callback_request(id, *lane))
            .collect();
        self.batch_gate.arm(events.into_iter().map(|(id, _)| id));
        self.state.resolver.invalidate(&self.channel);
        requests
    }
    pub fn callback_request(&self, id: &str, lane: usize) -> reqwest::RequestBuilder {
        let mut envelope = Envelope::new();
        envelope.callback["message_id"] = id.into();
        envelope.reply["inbound_message_id"] = id.into();
        envelope.payload["message_id"] = id.into();
        envelope.payload["conversation"]["id"] = format!("route-{lane}").into();
        envelope.reply["conversation_id"] = format!("route-{lane}").into();
        let (body, token) = envelope.signed();
        self.client
            .post(format!("{}/v1/callbacks/{}", self.base, self.channel))
            .header("content-type", "application/json")
            .header("x-nyxid-callback-token", token)
            .header("x-nyxid-platform", "telegram")
            .header("x-nyxid-message-id", id)
            .header("x-nyxid-timestamp", now_rfc3339())
            .body(body)
    }
    pub async fn quiet(&self) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while !self.tracker.is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
    pub async fn drain(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(5);
        assert!(self.tracker.drain(deadline).await);
        assert!(self.engine.take().unwrap().drain(deadline).await);
    }
}
fn definition(agent_key: &str) -> Value {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../cmaeg-core/tests/fixtures/channel.json"
    ))
    .unwrap();
    let mut profile = fixture["profile"].clone();
    profile["metadata"]["owner"] = json!({"kind":"human","subject":"alice"});
    json!({"schema_version":1,"name":"latency-budget","profile":profile,"provider":{"slug":"cma_codex"},
        "sources":[{"type":"nyxid_relay","issuer":"https://nyx.example","key_id":"key-1","route_ids":(0..LANES).map(|lane|format!("route-{lane}")).collect::<Vec<_>>(),"platform":"telegram","admission":{"type":"all"}}],
        "partition":{"version":1,"key":"conversation_and_sender"},
        "reply":{"auto_final":false,"ack_after_ms":0,"sinks":[{"type":"source"}]},
        "credentials":{"agent_key":agent_key,"channel_key":format!("nyx_route_{}",uuid::Uuid::new_v4().simple()),"hmac_secrets":[]}})
}
