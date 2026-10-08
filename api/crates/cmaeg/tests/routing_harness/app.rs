//! The production routing/turn/delivery composition, seeded below management.
use super::{
    fake_cma::FakeCma,
    fake_provider::{FakeProvider, FakeProviderConfig},
    peers::{serve, LocalWebhook, Peers},
    policy,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use cmaeg::{
    channels::resolver::{ChannelResolver, ResolvedChannel},
    delivery::{
        custom::CustomDelivery, engine::DeliveryEngine, relay_client::RelayClient,
        router::ShardedScheduler, sync::SyncRegistry, webhook_client::WebhookClient,
    },
    http::{self, events::CustomIngressState},
    ingress::IngressState,
    providers::{ProviderClient, ProviderRegistry},
    records::RecordClient,
    telegram::TelegramClient,
    tools::ToolsState,
    turns::{
        capacity::RoutedCapacity, fanout::FanoutLauncher, observer::NoopObserver, TurnRunner,
        TurnTracker,
    },
    DeliveryConfig, RoutingConfig, TurnConfig,
};
use cmaeg_core::{
    channel::ChannelDefinition,
    seal::{seal_blob, ChannelPointer, MasterKeyRing},
};
use serde_json::{json, Value};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::task::JoinHandle;

pub struct App {
    pub base: String,
    pub channel: Arc<ResolvedChannel>,
    pub state: IngressState,
    pub tools: ToolsState,
    pub providers: [FakeProvider; 2],
    pub peers: Peers,
    pub tracker: Arc<TurnTracker>,
    pub capacity: Arc<RoutedCapacity>,
    pub client: reqwest::Client,
    pub waiters: Arc<SyncRegistry>,
    cma: FakeCma,
    engine: Option<DeliveryEngine>,
    server: JoinHandle<()>,
}
impl Drop for App {
    fn drop(&mut self) {
        self.server.abort();
    }
}
impl App {
    pub async fn new(enabled: bool, edit: impl FnOnce(&mut Value)) -> Self {
        Self::configured(
            enabled,
            enabled,
            [FakeProviderConfig::default(), FakeProviderConfig::default()],
            edit,
        )
        .await
    }
    pub async fn configured(
        enabled: bool,
        delivery_enabled: bool,
        configs: [FakeProviderConfig; 2],
        edit: impl FnOnce(&mut Value),
    ) -> Self {
        let cma = FakeCma::start(false).await;
        let peers = Peers::start().await;
        let [main, reviewer] = configs;
        let providers = [
            FakeProvider::start(main).await,
            FakeProvider::start(reviewer).await,
        ];
        let keys =
            Arc::new(MasterKeyRing::parse(&format!("1:{}", STANDARD.encode([1; 32]))).unwrap());
        let records = Arc::new(RecordClient::new(&cma.base_url, "cma-service").unwrap());
        let resolver = ChannelResolver::new(records.clone(), keys.clone(), Default::default());
        let mut raw = policy::routed();
        raw["record_id"] = uuid::Uuid::new_v4().to_string().into();
        raw["sources"] = json!([
            policy::telegram(),
            policy::relay("telegram", "route-1"),
            policy::custom()
        ]);
        raw["sources"][1]["jwks_url"] = format!("{}/jwks", peers.base).into();
        raw["routing"]["default_consumer"]["name"] = "main".into();
        raw["routing"]["consumers"] =
            json!([policy::consumer(policy::REVIEWER, "reviewer", "nyxbot")]);
        raw["secrets"]["consumer_keys"] =
            json!([[policy::REVIEWER, policy::agent_key(policy::REVIEWER)]]);
        raw["reply"]["ack_after_ms"] = 0.into();
        raw["reply"]["sinks"] = json!([{"type":"source"},{"type":"sync"},
            {"type":"result_webhook","url":"https://receiver.example/events","hmac_key_id":"key-1"}]);
        edit(&mut raw);
        let channel = seed(&cma, &records, &keys, &resolver, raw).await;
        let entry = |slug: &str, provider: &FakeProvider| {
            json!({"slug":slug,"base_url":provider.base_url(),"kind":"responses_http","paths":{
            "agent_card":"/.well-known/agent-card.json","binding":"/api/v1/cmaeg/bindings/{binding_id}",
            "conversation":"/api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}","responses":"/api/v2/responses"},"max_inflight":256})
        };
        let registry = ProviderRegistry::parse(
            &json!([
                entry("cma_codex", &providers[0]),
                entry("nyxbot", &providers[1])
            ])
            .to_string(),
        )
        .unwrap();
        let clients = Arc::new(
            ProviderClient::new(
                registry,
                Duration::from_secs(2),
                Duration::from_secs(5),
                256,
            )
            .unwrap(),
        );
        let tracker = TurnTracker::new(http::readiness::Readiness::initialized());
        let capacity = Arc::new(RoutedCapacity::new(Default::default()));
        let (sender, outcomes) = tokio::sync::mpsc::channel(256);
        let delivery = Arc::new(ShardedScheduler::new(sender));
        let waiters = Arc::new(SyncRegistry::default());
        let telegram = TelegramClient::new(&peers.base, Duration::from_secs(2)).unwrap();
        let mut custom = CustomDelivery::new("https://gateway.example".into(), waiters.clone());
        custom.webhooks = WebhookClient::with_transport(Arc::new(LocalWebhook(peers.base.clone())));
        let engine = DeliveryEngine::start(
            outcomes,
            keys.clone(),
            RelayClient::new(&peers.base, Duration::from_secs(2))
                .unwrap()
                .with_telegram(telegram.clone()),
            DeliveryConfig {
                multi_consumer_enabled: delivery_enabled,
                ..Default::default()
            },
            tracker.clone(),
            custom,
        );
        let config = TurnConfig {
            turn_timeout: Duration::from_secs(10),
            ..Default::default()
        };
        let runner = Arc::new(TurnRunner::new(
            clients.clone(),
            tracker.clone(),
            delivery.clone(),
            config.clone(),
        ));
        let launcher = Arc::new(FanoutLauncher::new(
            runner,
            keys.clone(),
            delivery.clone(),
            capacity.clone(),
            tracker.clone(),
            config.turn_timeout,
            Arc::new(NoopObserver),
        ));
        let state = IngressState::new(
            resolver,
            keys.clone(),
            Default::default(),
            launcher,
            delivery.clone(),
        )
        .unwrap()
        .with_telegram(Arc::new(telegram))
        .with_routing(
            RoutingConfig {
                multi_consumer_enabled: enabled,
                non_human_providers: ["cma_codex".into(), "nyxbot".into()].into(),
                ..Default::default()
            },
            Arc::new(NoopObserver),
        );
        let tools = ToolsState {
            resolver: state.resolver.clone(),
            keys,
            providers: clients,
            delivery,
            tracker: tracker.clone(),
            public_origin: "https://gateway.example".into(),
            media: None,
            downloads: Default::default(),
            multi_consumer_enabled: delivery_enabled,
        };
        let (base, server) = serve(
            http::callbacks::router(state.clone())
                .merge(http::telegram::router(state.clone()))
                .merge(http::events::router(CustomIngressState {
                    ingress: state.clone(),
                    waiters: waiters.clone(),
                }))
                .merge(http::tools::router(tools.clone()))
                .merge(cmaeg::mcp::router(tools.clone())),
        )
        .await;
        Self {
            base,
            channel,
            state,
            tools,
            providers,
            peers,
            tracker,
            capacity,
            client: reqwest::Client::new(),
            waiters,
            cma,
            engine: Some(engine),
            server,
        }
    }
    pub async fn seed(&self, raw: Value) -> Arc<ResolvedChannel> {
        let records = RecordClient::new(&self.cma.base_url, "cma-service").unwrap();
        seed(
            &self.cma,
            &records,
            &self.state.master_keys,
            &self.state.resolver,
            raw,
        )
        .await
    }
    pub fn turns(&self, consumer: usize) -> Vec<Value> {
        self.providers[consumer]
            .requests()
            .into_iter()
            .filter(|r| r.method == "POST")
            .map(|r| serde_json::from_str(&r.body).unwrap())
            .collect()
    }
    pub async fn quiet(&self) {
        tokio::time::timeout(Duration::from_secs(12), async {
            while !self.tracker.is_empty() {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .expect("provider turns finish");
    }
    pub async fn drain(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(12);
        assert!(self.tracker.drain(deadline).await);
        assert!(self.engine.take().unwrap().drain(deadline).await);
    }
}
async fn seed(
    cma: &FakeCma,
    records: &RecordClient,
    keys: &MasterKeyRing,
    resolver: &ChannelResolver,
    raw: Value,
) -> Arc<ResolvedChannel> {
    let definition: ChannelDefinition = serde_json::from_value(raw).unwrap();
    definition.validate().unwrap();
    let pointer = ChannelPointer {
        v: 1,
        record_id: definition.record_id.clone(),
        record_url: records
            .record_url(definition.record_id.as_str())
            .unwrap()
            .to_string(),
    };
    let id = pointer.seal_id(keys).unwrap();
    cma.seed(
        definition.record_id.as_str(),
        &definition.owner.subject,
        seal_blob(keys, definition.to_blob_json().to_string().as_bytes()).unwrap(),
    );
    resolver.resolve(id.as_str()).await.unwrap()
}
