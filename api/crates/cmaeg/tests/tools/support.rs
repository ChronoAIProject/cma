use super::{
    fake_provider::FakeProvider,
    relay_support::{Envelope, Fixture},
};
use axum::{routing::get, Json, Router};
use cmaeg::{
    delivery::intents::{
        DeliveryCommand, DeliveryOutcome, IntentReceipt, TypingOutcome, TypingReceipt,
    },
    http,
    ingress::launch::LaunchRequest,
    mcp,
    providers::{EventContextSnapshot, ProviderClient, ProviderRegistry, TurnRequest},
    tools::ToolsState,
    turns::TurnTracker,
};
use cmaeg_core::{
    ids::MessageRef,
    message::{OutboundMessage, RenderingOutcome, RenderingReport},
    seal::{EventRef, MessageRefPayload},
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::{sync::mpsc, task::JoinHandle};
use utoipa::OpenApi;

pub struct App {
    pub fixture: Fixture,
    pub fake: FakeProvider,
    pub state: ToolsState,
    pub base: String,
    pub key: String,
    pub event_ref: EventRef,
    pub message_ref: MessageRef,
    pub requests: Arc<Mutex<Vec<OutboundMessage>>>,
    pub delay: Arc<AtomicU64>,
    tasks: Vec<JoinHandle<()>>,
}
impl Drop for App {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}
impl App {
    pub async fn new() -> Self {
        Self::with_text("hello").await
    }
    pub async fn with_text(text: &str) -> Self {
        let key = format!("nyxid_ag_{}", uuid::Uuid::new_v4().simple());
        let fixture = Fixture::new(
            |value| {
                value["secrets"]["agent_key_hash"] =
                    format!("sha256:{:x}", Sha256::digest(key.as_bytes())).into();
                value["secrets"]["provider_credential"] = key.clone().into();
            },
            Default::default(),
        )
        .await;
        let mut envelope = Envelope::new();
        envelope.group();
        envelope.payload["content"]["text"] = text.into();
        envelope.payload["raw_platform_data"]["message"]["text"] = text.into();
        assert_eq!(fixture.send(&envelope).await.status(), 202);
        let request = fixture.recorder.requests.lock().unwrap().pop().unwrap();
        let event_ref = request.event_ref.clone();
        let payload = event_ref
            .open(&fixture.state.master_keys, super::relay_support::now())
            .unwrap()
            .payload;
        let message_ref = MessageRef::seal(
            &MessageRefPayload {
                channel_id: fixture.id.clone(),
                event_id: payload.event_id.clone(),
                platform_message_id: "platform-1".into(),
                route_id: "route-1".into(),
                interaction_record_id: None,
            },
            &fixture.state.master_keys,
        )
        .unwrap();
        let fake = FakeProvider::start(Default::default()).await;
        let registry=ProviderRegistry::parse(&json!([{"slug":"cma_codex","base_url":fake.base_url(),"kind":"responses_http","paths":{"agent_card":"/.well-known/agent-card.json","binding":"/api/v1/cmaeg/bindings/{binding_id}","conversation":"/api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}","responses":"/api/v2/responses"},"max_inflight":1}]).to_string()).unwrap();
        let providers = Arc::new(
            ProviderClient::new(registry, Duration::from_secs(1), Duration::from_secs(1), 1)
                .unwrap(),
        );
        store_context(&providers, request).await;
        let (delivery, mut commands) = mpsc::channel::<DeliveryCommand>(16);
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recording = requests.clone();
        let delay = Arc::new(AtomicU64::new(0));
        let delayed = delay.clone();
        let returned_ref = message_ref.clone();
        let engine = tokio::spawn(async move {
            while let Some(command) = commands.recv().await {
                tokio::time::sleep(Duration::from_millis(delayed.load(Ordering::SeqCst))).await;
                match command {
                    DeliveryCommand::Reply {
                        message, receipt, ..
                    } => {
                        recording.lock().unwrap().push(message);
                        let _ = receipt.send(IntentReceipt {
                            message_ref: Some(returned_ref.clone()),
                            rendering: RenderingReport {
                                outcome: RenderingOutcome::Rendered,
                                blocks: vec![],
                            },
                            delivery: DeliveryOutcome::Confirmed {
                                platform_message_id: Some("platform-1".into()),
                            },
                        });
                    }
                    DeliveryCommand::Update { receipt, .. } => {
                        let _ = receipt.send(IntentReceipt {
                            message_ref: None,
                            rendering: RenderingReport {
                                outcome: RenderingOutcome::Unsupported,
                                blocks: vec![],
                            },
                            delivery: DeliveryOutcome::Unsupported,
                        });
                    }
                    DeliveryCommand::Typing { receipt, .. } => {
                        let _ = receipt.send(TypingReceipt {
                            outcome: TypingOutcome::Unsupported,
                        });
                    }
                    _ => panic!("unexpected stream command"),
                }
            }
        });
        let state = ToolsState {
            resolver: fixture.state.resolver.clone(),
            keys: fixture.state.master_keys.clone(),
            providers,
            delivery,
            tracker: TurnTracker::new(http::readiness::Readiness::initialized()),
            public_origin: "https://gateway.example".into(),
        };
        let router = http::tools::router(state.clone())
            .merge(mcp::router(state.clone()))
            .route(
                "/openapi.json",
                get(|| async { Json(cmaeg::ApiDoc::openapi()) }),
            );
        let (base, server) = serve(router).await;
        Self {
            fixture,
            fake,
            state,
            base,
            key,
            event_ref,
            message_ref,
            requests,
            delay,
            tasks: vec![engine, server],
        }
    }
    pub fn event_url(&self, operation: &str) -> String {
        format!(
            "{}/v1/events/{}/{operation}",
            self.base,
            self.event_ref.as_str()
        )
    }
    pub async fn rpc(&self, request: Value) -> reqwest::Response {
        self.fixture
            .client
            .post(format!("{}/mcp", self.base))
            .bearer_auth(&self.key)
            .json(&request)
            .send()
            .await
            .unwrap()
    }
}
async fn store_context(provider: &ProviderClient, request: LaunchRequest) {
    let snapshot = EventContextSnapshot {
        schema_version: 1,
        channel_id: request.channel.id.clone(),
        conversation_id: request.conversation_id.clone(),
        activity: request.activity,
        event_ref: request.event_ref,
    };
    let mut stream = provider
        .start_turn(
            &request.channel.definition.provider.slug,
            TurnRequest {
                timing: None,
                conversation_id: request.conversation_id,
                input: vec![],
                idempotency_key: request.idempotency_key,
                metadata: BTreeMap::new(),
                agent_key: request
                    .channel
                    .definition
                    .secrets
                    .provider_credential
                    .clone()
                    .unwrap(),
                event_context: Some(snapshot),
            },
        )
        .await
        .unwrap();
    (&mut stream.accepted).await.unwrap();
    while stream.events.recv().await.is_some() {}
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
pub async fn problem(response: reqwest::Response, status: u16, code: &str) -> Value {
    assert_eq!(response.status().as_u16(), status);
    assert_eq!(response.headers()["cmaeg-protocol-version"], "1");
    assert_eq!(
        response.headers()["content-type"],
        "application/problem+json"
    );
    let value: Value = response.json().await.unwrap();
    assert_eq!(value["code"], code);
    value
}
