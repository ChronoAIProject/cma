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
    delivery::{engine::DeliveryEngine, relay_client::RelayClient},
    http,
    ingress::IngressState,
    providers::{ProviderClient, ProviderRegistry},
    records::RecordClient,
    tools::ToolsState,
    turns::{TurnRunner, TurnTracker},
    ChannelCacheConfig, ManagementConfig,
};
use cmaeg_core::{channel::Secret, seal::MasterKeyRing};
use serde_json::{json, Value};
use std::{
    num::NonZeroU32,
    sync::{atomic::Ordering, Arc},
    time::{Duration, Instant},
};

pub const LANES: usize = 16;
pub struct App {
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
        let state = ManagementState::new(
            providers.clone(),
            Arc::new(RecordClient::new(&cma.base_url, "cma-service").unwrap()),
            Arc::new(CreatorAuthenticator::new(&identity.base).unwrap()),
            ManagementConfig {
                master_keys: keys.clone(),
                public_url: "http://127.0.0.1:8090".into(),
                cma_base_url: cma.base_url.clone(),
                tools_nyxid_slug: "cmaeg".into(),
                cma_service_token: Secret::new("cma-service"),
                nyxid_base_url: identity.base.clone(),
            },
            ChannelCacheConfig {
                restores_per_minute: NonZeroU32::new(60_000).unwrap(),
                ..Default::default()
            },
        );
        let tracker = TurnTracker::new(state.readiness.clone());
        let (delivery, outcomes) = tokio::sync::mpsc::channel(256);
        let waiters = Arc::default();
        let engine = DeliveryEngine::start(
            outcomes,
            keys.clone(),
            RelayClient::new(&identity.base, Duration::from_secs(5)).unwrap(),
            Default::default(),
            tracker.clone(),
            cmaeg::delivery::custom::CustomDelivery::new("http://127.0.0.1:8090".into(), waiters),
        );
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
        .unwrap();
        let tools = ToolsState {
            resolver: state.resolver.clone(),
            keys,
            providers,
            delivery,
            tracker: tracker.clone(),
            public_origin: "http://127.0.0.1:8090".into(),
            media: None,
        };
        let router = cmaeg::router_with_state(state.clone())
            .merge(http::callbacks::router(ingress))
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
        Self {
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
            .send()
            .await
            .unwrap()
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
