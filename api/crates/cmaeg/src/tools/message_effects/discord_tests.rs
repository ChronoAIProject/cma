//! Exercise the real delivery engine, public effects routes and durable receipts.
use super::*;
use crate::channels::{definition::fixture as definition_fixture, routing_fixture};
use crate::tools::fake_cma;
use crate::{
    auth::AgentCaller,
    channels::resolver::ChannelResolver,
    delivery::{
        custom::CustomDelivery,
        engine::DeliveryEngine,
        intents::{DeliveryCommand, ReplySource},
        relay_client::RelayClient,
        router::ShardedScheduler,
    },
    discord::rest::effects_tests::Fixture as Platform,
    http,
    providers::{ProviderClient, ProviderRegistry},
    records::RecordClient,
    turns::TurnTracker,
    DeliveryConfig,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use cmaeg_core::{
    activity::{Activity, SourceKind},
    channel::{AdmissionPolicy, ChannelDefinition, Source},
    seal::{
        event_ref::activity_digest, seal_blob, ChannelPointer, EventRef, EventRefPayload,
        MasterKeyRing, ReplyAddress,
    },
};
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::time::Duration;
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
};

#[path = "discord_media_tests.rs"]
mod media;
#[path = "native_media_edit_tests.rs"]
mod media_edits;

struct App {
    backend: fake_cma::FakeCma,
    platform: Platform,
    state: ToolsState,
    definition: ChannelDefinition,
    event: EventRefPayload,
    activity: Activity,
    key: String,
    base: String,
    engine: Option<DeliveryEngine>,
    server: Option<JoinHandle<()>>,
    media_effects: bool,
}
impl Drop for App {
    fn drop(&mut self) {
        if let Some(server) = self.server.take() {
            server.abort();
        }
    }
}
impl App {
    async fn new() -> Self {
        let backend = fake_cma::FakeCma::start(false).await;
        let platform = Platform::new().await;
        let records = RecordClient::new(&backend.base_url, "cma-service").unwrap();
        let keys =
            Arc::new(MasterKeyRing::parse(&format!("1:{}", STANDARD.encode([3; 32]))).unwrap());
        let key = format!("nyxid_ag_{}", uuid::Uuid::new_v4().simple());
        let mut definition = definition_fixture::with_agent_key(&key);
        definition.record_id = uuid::Uuid::new_v4().to_string().parse().unwrap();
        definition.sources = vec![Source::Discord {
            thread_history: false,
            thread_lifecycle: false,
            conversation_history: false,
            bot_id: "77".into(),
            intents: 37377,
            admission: AdmissionPolicy::All {},
        }];
        definition.secrets.discord_bot_token = Some(platform.token.clone());
        definition.partition.key = cmaeg_core::channel::PartitionKey::ConversationAndSender;
        definition.validate().unwrap();
        let pointer = ChannelPointer {
            v: 1,
            record_id: definition.record_id.clone(),
            record_url: records
                .record_url(definition.record_id.as_str())
                .unwrap()
                .to_string(),
        };
        let id = pointer.seal_id(&keys).unwrap();
        backend.seed(
            definition.record_id.as_str(),
            &definition.owner.subject,
            seal_blob(
                &keys,
                &serde_json::to_vec(&definition.to_blob_json()).unwrap(),
            )
            .unwrap(),
        );
        let mut activity: Activity = serde_json::from_str(include_str!(
            "../../../../cmaeg-core/tests/fixtures/activity.json"
        ))
        .unwrap();
        activity.source = SourceKind::Discord {
            bot_id: "77".into(),
        };
        activity.conversation.id = "99".into();
        activity.event_id = "discord:77:700".parse().unwrap();
        let event = EventRefPayload {
            v: 1,
            telegram_admission: None,
            activity_sha256: Some(activity_digest(&activity)),
            channel_id: id,
            record_id: definition.record_id.clone(),
            event_id: activity.event_id.clone(),
            partition_key: "discord:99".into(),
            conversation_id: "conversation-1".parse().unwrap(),
            expires_at: now() + 600,
            reply: ReplyAddress::Discord {
                thread_id: None,
                bot_id: "77".into(),
                channel_id: "99".into(),
                message_id: "700".into(),
            },
            consumer: None,
            lineage: None,
        };
        let (sender, _) = mpsc::channel(16);
        let state = ToolsState {
            discord_thread_history: None,
            telegram_topics: None,
            discord_threads: None,
            native_lark_conversation_history: None,
            discord_conversation_history: None,
            complete_replies_enabled: false,
            commonmark_relay: Default::default(),
            discord_interactions_enabled: false,
            native_discord_media_controls_enabled: false,
            native_discord_media_card_edits_enabled: false,
            native_discord_media_card_replacements_enabled: false,
            native_discord_media_card_sets_enabled: false,
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
            whatsapp_templates_enabled: false,
            whatsapp_interactions_enabled: false,
            native_whatsapp_interactions_enabled: false,
            native_whatsapp_media_controls_enabled: false,
            consumer_interactions_enabled: false,
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
            relay_text_edits_enabled: false,
            native_discord_edits_enabled: true,
            discord_effects_issuance_enabled: true,
            discord_effects: Some(Arc::new(discord::DiscordEffectsClient::new(
                platform.api.clone(),
            ))),
            lark_cards_enabled: false,
            resolver: ChannelResolver::new(Arc::new(records), keys.clone(), Default::default()),
            keys,
            providers: Arc::new(
                ProviderClient::new(
                    ProviderRegistry::parse("[]").unwrap(),
                    Duration::from_secs(1),
                    Duration::from_secs(1),
                    1,
                )
                .unwrap(),
            ),
            delivery: Arc::new(ShardedScheduler::new(sender)),
            routed_replies: None,
            tracker: TurnTracker::new(http::readiness::Readiness::initialized()),
            public_origin: "https://gateway.example".into(),
            public_api_url: "https://gateway.example".into(),
            public_api_aliases: vec![],
            advertise_public_api_url: true,
            media: None,
            downloads: Default::default(),
        };
        let mut app = Self {
            backend,
            platform,
            state,
            definition,
            event,
            activity,
            key,
            base: String::new(),
            engine: None,
            server: None,
            media_effects: false,
        };
        app.restart().await;
        app
    }
    async fn restart(&mut self) {
        if let Some(server) = self.server.take() {
            server.abort();
        }
        self.engine.take();
        self.state.resolver = ChannelResolver::new(
            Arc::new(RecordClient::new(&self.backend.base_url, "cma-service").unwrap()),
            self.state.keys.clone(),
            Default::default(),
        );
        let (sender, receiver) = mpsc::channel(16);
        let mut transport = RelayClient::new("http://127.0.0.1:1", Duration::from_secs(1))
            .unwrap()
            .with_native_discord_edits(self.state.native_discord_edits_enabled)
            .with_native_discord_effects(self.state.discord_effects_issuance_enabled)
            .with_native_discord_media_effects(self.media_effects);
        transport.discord = self.platform.api.clone();
        if let Some(media) = &self.state.native_media_edits {
            transport = transport
                .with_telegram((*media.telegram).clone())
                .with_telegram_effects(true)
                .with_telegram_media_effects(true)
                .with_interactions(Arc::new(crate::interactions::InteractionStore::new(
                    RecordClient::new(&self.backend.base_url, "cma-service").unwrap(),
                    self.state.keys.clone(),
                    self.state.providers.clone(),
                )))
                .with_native_media_edits(
                    media.telegram_issuance,
                    media.discord_issuance,
                    Arc::new(crate::native_media::store::Store::new(
                        self.state.resolver.record_client(),
                        self.state.keys.clone(),
                    )),
                );
        }
        self.state.delivery = Arc::new(ShardedScheduler::new(sender));
        self.engine = Some(DeliveryEngine::start(
            receiver,
            self.state.keys.clone(),
            transport,
            DeliveryConfig::default(),
            self.state.tracker.clone(),
            CustomDelivery::new(self.state.public_origin.clone(), Arc::default()),
        ));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        self.base = format!("http://{}", listener.local_addr().unwrap());
        let router =
            http::tools::router(self.state.clone()).merge(crate::mcp::router(self.state.clone()));
        self.server = Some(tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap()
        }));
    }
    async fn send(&self) -> crate::delivery::intents::IntentReceipt {
        self.send_message(serde_json::from_value(json!({"text":"hello"})).unwrap())
            .await
    }
    async fn send_message(
        &self,
        message: cmaeg_core::message::OutboundMessage,
    ) -> crate::delivery::intents::IntentReceipt {
        let (receipt, received) = oneshot::channel();
        let channel = self
            .state
            .resolver
            .resolve_current(self.event.channel_id.as_str())
            .await
            .unwrap();
        self.state
            .delivery
            .submit(DeliveryCommand::Reply {
                trace: tracing::Span::none(),
                channel,
                event: self.event.clone(),
                message,
                source: ReplySource::Tool,
                receipt,
            })
            .await
            .unwrap();
        received.await.unwrap()
    }
    async fn reference(&self) -> MessageRef {
        let sent = self.send().await;
        assert_eq!(sent.delivery.kind(), "confirmed");
        sent.message_ref.unwrap()
    }
    fn effect(
        &self,
        reference: &MessageRef,
        op: &str,
        key: &str,
        body: Value,
    ) -> reqwest::RequestBuilder {
        reqwest::Client::new()
            .post(format!(
                "{}/v1/messages/{}/{op}",
                self.base,
                reference.as_str()
            ))
            .bearer_auth(&self.key)
            .header("idempotency-key", key)
            .json(&body)
    }
    async fn caps(&self, reference: &MessageRef) -> Value {
        value(
            reqwest::Client::new()
                .get(format!(
                    "{}/v1/messages/{}/capabilities",
                    self.base,
                    reference.as_str()
                ))
                .bearer_auth(&self.key)
                .send()
                .await
                .unwrap(),
        )
        .await
    }
    async fn update(&mut self) {
        let previous = self.definition.version;
        self.definition.version += 1;
        self.state
            .resolver
            .record_client()
            .put(
                &self.definition.record_id,
                &self.definition.owner.subject,
                &seal_blob(
                    &self.state.keys,
                    &serde_json::to_vec(&self.definition.to_blob_json()).unwrap(),
                )
                .unwrap(),
                Some(previous),
            )
            .await
            .unwrap();
    }
}
async fn value(response: reqwest::Response) -> Value {
    let status = response.status();
    let body: Value = response.json().await.unwrap();
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

mod contracts;
