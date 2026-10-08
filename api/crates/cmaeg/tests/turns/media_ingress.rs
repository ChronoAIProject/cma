//! Router-level ingress over a real turn runner with inbound media on, for
//! the relay, native Telegram and custom transports.
use crate::{
    fake_provider::{FakeProvider, FakeProviderConfig, ReceivedRequest},
    relay_support::{now, now_rfc3339, Envelope, Fixture, ISSUER},
};
use cmaeg::{
    auth::webhook,
    delivery::sync::SyncRegistry,
    http::{self, events::CustomIngressState, readiness::Readiness},
    ingress::IngressState,
    providers::{ProviderClient, ProviderRegistry},
    turns::{TurnRunner, TurnTracker},
    IngressConfig, TurnConfig,
};
use cmaeg_core::channel::Secret;
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};
use tokio::{sync::mpsc, task::JoinHandle};

pub const WEBHOOK_SECRET: &str = "native-webhook-fixture";
const ORIGIN: &str = "https://files.example";
/// Far above any test's provider delay, so a delayed start is never a timeout.
const PROVIDER_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const PROVIDER_CAPACITY: usize = 512;

/// The transport the channel's single source serves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transport {
    Relay,
    Native,
    Custom,
}

pub struct MediaApp {
    pub fixture: Fixture,
    pub provider: FakeProvider,
    pub ingress: IngressState,
    pub runner: Arc<TurnRunner>,
    pub tracker: Arc<TurnTracker>,
    pub base: String,
    secret: Secret,
    tasks: Vec<JoinHandle<()>>,
}

impl Drop for MediaApp {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

impl MediaApp {
    pub async fn new(
        transport: Transport,
        provider: FakeProviderConfig,
        turns: TurnConfig,
        ingress: IngressConfig,
    ) -> Self {
        let secret = Secret::new(uuid::Uuid::new_v4().to_string());
        let hmac = secret.as_str().to_owned();
        let fixture = Fixture::with_media(
            move |value| {
                value["secrets"]["provider_credential"] = "provider-fixture".into();
                match transport {
                    Transport::Relay => {}
                    Transport::Native => {
                        value["sources"] = json!([{"type":"telegram","bot_id":"77","bot_username":"fixture_bot","admission":{"type":"all"}}]);
                        value["secrets"]["telegram_bot_token"] = "77:fixture".into();
                        value["secrets"]["telegram_webhook_secret"] = WEBHOOK_SECRET.into();
                    }
                    Transport::Custom => {
                        value["sources"] = json!([{"type":"custom","auth":{"type":"hmac","key_id":"ingress"},
                            "attachments":{"pointer":"/attachments","origins":[ORIGIN]}}]);
                        value["partition"]["key"] = "per_event".into();
                        value["reply"]["sinks"] = json!([{"type":"sync"}]);
                        value["secrets"]["hmac_secrets"] = json!([["ingress", hmac]]);
                    }
                }
            },
            IngressConfig::default(),
        )
        .await;
        let provider = FakeProvider::start(provider).await;
        let registry = ProviderRegistry::parse(&json!([{"slug":"cma_codex","base_url":provider.base_url(),"kind":"responses_http","paths":{"agent_card":"/.well-known/agent-card.json","binding":"/api/v1/cmaeg/bindings/{binding_id}","conversation":"/api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}","responses":"/api/v2/responses"},"max_inflight":PROVIDER_CAPACITY}]).to_string()).unwrap();
        let providers = Arc::new(
            ProviderClient::new(
                registry,
                Duration::from_secs(1),
                PROVIDER_REQUEST_TIMEOUT,
                PROVIDER_CAPACITY,
            )
            .unwrap(),
        );
        let tracker = TurnTracker::new(Readiness::initialized());
        // Outcomes are not under test; a full channel would stall the turns.
        let (delivery, mut outcomes) = mpsc::channel(256);
        let sink = tokio::spawn(async move { while outcomes.recv().await.is_some() {} });
        let delivery = Arc::new(cmaeg::delivery::router::ShardedScheduler::new(delivery));
        let runner = Arc::new(TurnRunner::new(providers, tracker.clone(), delivery, turns));
        let state = IngressState::new(
            fixture.state.resolver.clone(),
            fixture.state.master_keys.clone(),
            ingress,
            runner.clone(),
        )
        .unwrap()
        .with_media_enabled(true);
        let router = http::callbacks::router(state.clone())
            .merge(http::telegram::router(state.clone()))
            .merge(http::events::router(CustomIngressState {
                ingress: state.clone(),
                waiters: Arc::new(SyncRegistry::default()),
            }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Self {
            fixture,
            provider,
            ingress: state,
            runner,
            tracker,
            base,
            secret,
            tasks: vec![sink, server],
        }
    }

    /// Send a signed relay callback.
    pub async fn relay(&self, signed: &(Vec<u8>, String)) -> reqwest::Response {
        self.fixture
            .client
            .post(format!("{}/v1/callbacks/{}", self.base, self.fixture.id))
            .header("content-type", "application/json")
            .header("x-nyxid-callback-token", &signed.1)
            .header("x-nyxid-platform", "telegram")
            .header("x-nyxid-message-id", "message-1")
            .header("x-nyxid-timestamp", now_rfc3339())
            .body(signed.0.clone())
            .send()
            .await
            .unwrap()
    }

    /// Send a native Telegram update with the channel's webhook secret.
    pub async fn native(&self, update: &Value) -> reqwest::Response {
        self.fixture
            .client
            .post(format!("{}/v1/telegram/{}", self.base, self.fixture.id))
            .header("x-telegram-bot-api-secret-token", WEBHOOK_SECRET)
            .json(update)
            .send()
            .await
            .unwrap()
    }

    /// Send an HMAC-signed structured custom event.
    pub async fn custom(&self, event: &Value) -> reqwest::Response {
        let body = event.to_string();
        let timestamp = now().to_string();
        let signature = webhook::signature(&self.secret, "delivery-1", &timestamp, body.as_bytes());
        self.fixture
            .client
            .post(format!(
                "{}/v1/channels/{}/events",
                self.base, self.fixture.id
            ))
            .header("content-type", "application/cloudevents+json")
            .header("webhook-id", "delivery-1")
            .header("webhook-timestamp", timestamp)
            .header("webhook-signature", signature)
            .body(body)
            .send()
            .await
            .unwrap()
    }

    /// Provider turn requests received so far, oldest first.
    pub fn turns(&self) -> Vec<ReceivedRequest> {
        self.provider
            .requests()
            .into_iter()
            .filter(|request| request.method == "POST")
            .collect()
    }

    /// Wait until `count` provider turns have arrived.
    pub async fn wait_for_turns(&self, count: usize) {
        tokio::time::timeout(Duration::from_secs(30), async {
            while self.turns().len() < count {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("expected {count} turns, saw {}", self.turns().len()));
    }
}

/// A signed relay callback: a photo when `photo`, otherwise text, from a
/// sender of its own so that each sender is its own conversation.
pub fn relay_message(sender: u64, photo: bool) -> (Vec<u8>, String) {
    let mut envelope = Envelope::new();
    envelope.payload["sender"]["platform_id"] = sender.to_string().into();
    let message = &mut envelope.payload["raw_platform_data"]["message"];
    message["from"]["id"] = sender.into();
    if photo {
        message.as_object_mut().unwrap().remove("text");
        message["photo"] = json!([{"file_id":"AgACAgIAAxkBAAIBX2Zx-turn","file_unique_id":"AgADturn",
            "width":1280,"height":960,"file_size":81234}]);
        envelope.payload["content"] = json!({"type":"image","attachments":[{
            "download_url":format!("{ISSUER}/api/v1/channel-relay/messages/message-1/attachments/0"),
            "content_type":"image","url":"AgACAgIAAxkBAAIBX2Zx-turn","size_bytes":81234}]});
    }
    envelope.signed()
}

/// A native Telegram update: a photo when `photo`, otherwise text.
pub fn native_update(photo: bool) -> Value {
    let mut update = json!({"update_id":101,"message":{"message_id":5,"date":now()-5,
        "chat":{"id":42,"type":"private"},"from":{"id":42,"is_bot":false,"first_name":"Alice"},"text":"hi"}});
    if photo {
        let message = update["message"].as_object_mut().unwrap();
        message.remove("text");
        message.insert(
            "photo".into(),
            json!([{"file_id":"BAACAgIA-AgADturn","file_unique_id":"AgADturn","width":90,"height":90}]),
        );
    }
    update
}

/// A custom event: one declared PDF when `media`, otherwise plain data.
pub fn custom_event(id: &str, media: bool) -> Value {
    let data = if media {
        json!({"attachments":[{"url":format!("{ORIGIN}/report.pdf"),"mime_type":"application/pdf","size_bytes":4096}]})
    } else {
        json!({"order":7})
    };
    json!({"specversion":"1.0","id":id,"type":"order.created",
        "source":"https://orders.example","subject":"customer-1","data":data})
}

/// A message carrying `attachments`, given in their wire form.
pub fn message_with(attachments: Value) -> cmaeg_core::activity::ActivityKind {
    cmaeg_core::activity::ActivityKind::Message {
        text: "look at this".into(),
        attachments: serde_json::from_value(attachments).unwrap(),
        mentions_bot: true,
        reply_to: None,
        album_id: None,
    }
}

/// A relayed message carrying one retrievable photo, as the turn fixture's
/// relay activity would.
pub fn photo() -> cmaeg_core::activity::ActivityKind {
    message_with(
        json!([{"reference":"telegram:AgADturn","kind":"image","variant":"photo",
        "mime_type":"image/jpeg","source":{"type":"nyxid_relay","index":0}}]),
    )
}
