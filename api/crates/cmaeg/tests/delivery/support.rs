use super::{
    fake_nyxid_reply::{FakeReply, Reply},
    fake_provider::{FakeProvider, FakeProviderConfig},
    relay_support::{Envelope, Fixture},
};
use cmaeg::{
    delivery::{
        engine::DeliveryEngine,
        intents::{DeliveryCommand, IntentReceipt, ReplySource},
        relay_client::RelayClient,
    },
    http,
    ingress::{launch::LaunchRequest, IngressState},
    interactions::InteractionStore,
    providers::{ProviderClient, ProviderRegistry},
    records::RecordClient,
    tools::ToolsState,
    turns::{outcomes::TurnOutcome, TurnRunner, TurnTracker},
    DeliveryConfig, TurnConfig,
};
use cmaeg_core::{message::OutboundMessage, seal::EventRefPayload};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
};
pub struct App {
    pub fake: FakeReply,
    pub fixture: Fixture,
    pub provider: FakeProvider,
    pub turn: Arc<LaunchRequest>,
    pub event: EventRefPayload,
    pub commands: mpsc::Sender<DeliveryCommand>,
    pub engine: Option<DeliveryEngine>,
    pub tracker: Arc<TurnTracker>,
    pub base: String,
    pub key: String,
    server: JoinHandle<()>,
}
impl Drop for App {
    fn drop(&mut self) {
        self.server.abort();
    }
}
impl App {
    pub async fn new(
        replies: Vec<Reply>,
        ack_ms: u32,
        config: DeliveryConfig,
        provider_config: FakeProviderConfig,
    ) -> Self {
        Self::build("telegram", false, replies, ack_ms, config, provider_config).await
    }
    /// A relay channel on `platform`. With `interactive`, the relay client holds an
    /// interaction store whose record service is unreachable, so any attempt to
    /// send native controls is visible as a rejection.
    pub async fn on(platform: &str, interactive: bool) -> Self {
        Self::build(
            platform,
            interactive,
            vec![],
            0,
            DeliveryConfig::default(),
            FakeProviderConfig::default(),
        )
        .await
    }
    async fn build(
        platform: &str,
        interactive: bool,
        replies: Vec<Reply>,
        ack_ms: u32,
        config: DeliveryConfig,
        provider_config: FakeProviderConfig,
    ) -> Self {
        let fake = FakeReply::start(replies).await;
        let key = format!("nyx_{}", uuid::Uuid::new_v4().simple());
        let mut fixture = Fixture::on(
            platform,
            |value| {
                value["reply"]["ack_after_ms"] = ack_ms.into();
                value["secrets"]["agent_key_hash"] =
                    format!("sha256:{:x}", Sha256::digest(key.as_bytes())).into();
                value["secrets"]["provider_credential"] = key.clone().into();
                value["secrets"]["channel_key"] = "test-route-credential".into();
            },
            Default::default(),
        )
        .await;
        assert_eq!(fixture.send(&Envelope::on(platform)).await.status(), 202);
        let turn = Arc::new(fixture.recorder.requests.lock().unwrap().pop().unwrap());
        let event = turn
            .event_ref
            .open(&fixture.state.master_keys, super::relay_support::now())
            .unwrap()
            .payload;
        let tracker = TurnTracker::new(http::readiness::Readiness::initialized());
        let (commands, receiver) = mpsc::channel(256);
        let provider = FakeProvider::start(provider_config).await;
        let registry=ProviderRegistry::parse(&json!([{"slug":"cma_codex","base_url":provider.base_url(),"kind":"responses_http","paths":{"agent_card":"/.well-known/agent-card.json","binding":"/api/v1/cmaeg/bindings/{binding_id}","conversation":"/api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}","responses":"/api/v2/responses"},"max_inflight":8}]).to_string()).unwrap();
        let providers = Arc::new(
            ProviderClient::new(registry, Duration::from_secs(1), Duration::from_secs(2), 8)
                .unwrap(),
        );
        let mut relay = RelayClient::new(&fake.base, config.reply_timeout).unwrap();
        if interactive {
            let records = RecordClient::new("http://127.0.0.1:9", "unused-service-bearer").unwrap();
            relay = relay.with_interactions(Arc::new(InteractionStore::new(
                records,
                fixture.state.master_keys.clone(),
                providers.clone(),
            )));
        }
        let engine = DeliveryEngine::start(
            receiver,
            fixture.state.master_keys.clone(),
            relay,
            config,
            tracker.clone(),
            cmaeg::delivery::custom::CustomDelivery::new(
                "https://gateway.example".into(),
                Arc::default(),
            ),
        );
        let runner = Arc::new(TurnRunner::new(
            providers.clone(),
            tracker.clone(),
            commands.clone(),
            TurnConfig::default(),
        ));
        let ingress = IngressState::new(
            fixture.state.resolver.clone(),
            fixture.state.master_keys.clone(),
            Default::default(),
            runner,
        )
        .unwrap();
        let tools = ToolsState {
            resolver: fixture.state.resolver.clone(),
            keys: fixture.state.master_keys.clone(),
            providers,
            delivery: commands.clone(),
            tracker: tracker.clone(),
            public_origin: "https://gateway.example".into(),
            media: None,
        };
        let router = http::callbacks::router(ingress).merge(http::tools::router(tools));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        fixture.url = format!("{base}/v1/callbacks/{}", fixture.id);
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Self {
            fake,
            fixture,
            provider,
            turn,
            event,
            commands,
            engine: Some(engine),
            tracker,
            base,
            key,
            server,
        }
    }
    pub async fn standard(ack_ms: u32) -> Self {
        Self::new(
            vec![],
            ack_ms,
            DeliveryConfig::default(),
            FakeProviderConfig::default(),
        )
        .await
    }
    pub async fn outcome(&self, outcome: TurnOutcome) {
        self.commands
            .send(DeliveryCommand::StreamOutcome {
                turn: self.turn.clone(),
                outcome,
            })
            .await
            .unwrap();
    }
    pub async fn reply(&self, value: Value) -> IntentReceipt {
        let message: OutboundMessage = serde_json::from_value(value).unwrap();
        let (receipt, received) = oneshot::channel();
        self.commands
            .send(DeliveryCommand::Reply {
                trace: tracing::Span::current(),
                channel: self.turn.channel.clone(),
                event: self.event.clone(),
                message,
                source: ReplySource::Tool,
                receipt,
            })
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(20), received)
            .await
            .unwrap()
            .unwrap()
    }
    pub async fn drain(&mut self) {
        assert!(
            self.engine
                .take()
                .unwrap()
                .drain(Instant::now() + Duration::from_secs(20))
                .await
        );
    }
}
