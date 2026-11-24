use super::{
    fake_cma::FakeCma,
    fake_nyxid_reply::{FakeReply, Reply},
    fake_provider::FakeProvider,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use cmaeg::{
    channels::resolver::ChannelResolver,
    delivery::{custom::CustomDelivery, engine::DeliveryEngine, relay_client::RelayClient},
    http,
    providers::{EventContextSnapshot, ProviderClient, ProviderRegistry, TurnRequest},
    records::RecordClient,
    tools::ToolsState,
    turns::TurnTracker,
    DeliveryConfig,
};
use cmaeg_core::{
    activity::Activity,
    channel::Secret,
    channel::{ChannelDefinition, Source},
    ids::{ChannelId, ConsumerId, ConversationId, IdempotencyKey},
    seal::{seal_blob, ChannelPointer, EventRef, EventRefPayload, MasterKeyRing, ReplyAddress},
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{sync::Arc, time::Duration};
use tokio::{sync::mpsc, task::JoinHandle};

pub struct App {
    pub backend: FakeCma,
    pub relay: FakeReply,
    _provider: FakeProvider,
    pub records: RecordClient,
    pub state: ToolsState,
    pub definition: ChannelDefinition,
    pub id: ChannelId,
    pub key: String,
    pub base: String,
    pub client: reqwest::Client,
    pub native: Option<cmaeg::telegram::TelegramClient>,
    pub telegram_media_effects: bool,
    engine: Option<DeliveryEngine>,
    server: JoinHandle<()>,
}
impl Drop for App {
    fn drop(&mut self) {
        self.server.abort();
    }
}
impl App {
    pub async fn new(replies: Vec<Reply>) -> Self {
        let backend = FakeCma::start(false).await;
        let relay = FakeReply::start(replies).await;
        let provider = FakeProvider::start(Default::default()).await;
        let records = RecordClient::new(&backend.base_url, "cma-service").unwrap();
        let keys =
            Arc::new(MasterKeyRing::parse(&format!("1:{}", STANDARD.encode([3; 32]))).unwrap());
        let key = format!("nyxid_ag_{}", uuid::Uuid::new_v4().simple());
        let mut value: Value = serde_json::from_str(include_str!(
            "../../../cmaeg-core/tests/fixtures/channel.json"
        ))
        .unwrap();
        value["record_id"] = uuid::Uuid::new_v4().to_string().into();
        value["sources"] = json!([{"type":"nyxid_relay","issuer":"https://nyx.example","jwks_url":"https://nyx.example/jwks",
            "key_id":"key-1","route_ids":["route-alice","route-bob"],"platform":"telegram","admission":{"type":"all"}}]);
        value["partition"]["key"] = "conversation_and_sender".into();
        value["secrets"]["agent_key_hash"] =
            format!("sha256:{:x}", Sha256::digest(key.as_bytes())).into();
        value["secrets"]["provider_credential"] = key.clone().into();
        value["secrets"]["channel_key"] = "fixture-channel-credential".into();
        let mut definition: ChannelDefinition = serde_json::from_value(value).unwrap();
        definition.profile_digest = definition.profile.digest();
        let id = ChannelPointer {
            v: 1,
            record_id: definition.record_id.clone(),
            record_url: records
                .record_url(definition.record_id.as_str())
                .unwrap()
                .to_string(),
        }
        .seal_id(&keys)
        .unwrap();
        records
            .put(
                &definition.record_id,
                &definition.owner.subject,
                &seal_blob(&keys, definition.to_blob_json().to_string().as_bytes()).unwrap(),
                None,
            )
            .await
            .unwrap();
        let registry = ProviderRegistry::parse(&json!([{"slug":"cma_codex","base_url":provider.base_url(),"kind":"responses_http",
            "paths":{"agent_card":"/.well-known/agent-card.json","binding":"/api/v1/cmaeg/bindings/{binding_id}",
            "conversation":"/api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}","responses":"/api/v2/responses"},"max_inflight":8}]).to_string()).unwrap();
        let providers = Arc::new(
            ProviderClient::new(registry, Duration::from_secs(1), Duration::from_secs(2), 8)
                .unwrap(),
        );
        let tracker = TurnTracker::new(http::readiness::Readiness::initialized());
        let (delivery, receiver) = mpsc::channel(16);
        let state = ToolsState {
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
            resolver: ChannelResolver::new(
                Arc::new(records.clone()),
                keys.clone(),
                Default::default(),
            ),
            keys: keys.clone(),
            providers,
            delivery: Arc::new(cmaeg::delivery::router::ShardedScheduler::new(delivery)),
            routed_replies: None,
            tracker: tracker.clone(),
            public_origin: "https://gateway.example".into(),
            public_api_url: "https://gateway.example".into(),
            public_api_aliases: vec![],
            advertise_public_api_url: true,
            media: None,
            downloads: Default::default(),
        };
        let engine = engine(receiver, &state, &relay, None, false);
        let (base, server) =
            serve(http::tools::router(state.clone()).merge(cmaeg::mcp::router(state.clone())))
                .await;
        Self {
            backend,
            relay,
            _provider: provider,
            records,
            state,
            definition,
            id,
            key,
            base,
            client: reqwest::Client::new(),
            native: None,
            telegram_media_effects: false,
            engine: Some(engine),
            server,
        }
    }
    pub async fn restart(&mut self) {
        self.server.abort();
        self.engine.take();
        let (delivery, receiver) = mpsc::channel(16);
        self.state.delivery = Arc::new(cmaeg::delivery::router::ShardedScheduler::new(delivery));
        self.state.resolver = ChannelResolver::new(
            Arc::new(self.records.clone()),
            self.state.keys.clone(),
            Default::default(),
        );
        self.engine = Some(engine(
            receiver,
            &self.state,
            &self.relay,
            self.native.clone(),
            self.telegram_media_effects,
        ));
        (self.base, self.server) = serve(
            http::tools::router(self.state.clone()).merge(cmaeg::mcp::router(self.state.clone())),
        )
        .await;
    }
    pub async fn event(&self, actor: &str, name: &str) -> EventRef {
        let key = self.definition.secrets.provider_credential.clone().unwrap();
        self.event_for(actor, name, None, key).await
    }
    /// An event issued to `consumer` (`None` = default), whose context the
    /// provider stores under that consumer's `credential`.
    pub async fn event_for(
        &self,
        actor: &str,
        name: &str,
        consumer: Option<ConsumerId>,
        credential: Secret,
    ) -> EventRef {
        self.event_for_with_thread(actor, name, consumer, credential, false)
            .await
    }
    pub async fn event_with_thread(&self, actor: &str, name: &str, threaded: bool) -> EventRef {
        let key = self.definition.secrets.provider_credential.clone().unwrap();
        self.event_for_with_thread(actor, name, None, key, threaded)
            .await
    }
    async fn event_for_with_thread(
        &self,
        actor: &str,
        name: &str,
        consumer: Option<ConsumerId>,
        credential: Secret,
        threaded: bool,
    ) -> EventRef {
        let mut activity: Activity = serde_json::from_str(include_str!(
            "../../../cmaeg-core/tests/fixtures/activity.json"
        ))
        .unwrap();
        activity.actor.id = actor.into();
        activity.conversation.id = format!("chat-{actor}");
        activity.event_id = name.parse().unwrap();
        activity.source = cmaeg_core::activity::SourceKind::NyxidRelay {
            platform: if threaded {
                cmaeg_core::channel::Platform::Lark
            } else {
                cmaeg_core::channel::Platform::Telegram
            },
            route_id: format!("route-{actor}"),
        };
        enum NativeSource {
            Telegram {
                bot_id: String,
            },
            Discord {
                bot_id: String,
            },
            Whatsapp {
                app_id: String,
                waba_id: String,
                phone_number_id: String,
            },
        }
        let native = self
            .definition
            .sources
            .iter()
            .find_map(|source| match source {
                Source::Telegram { bot_id, .. } => Some(NativeSource::Telegram {
                    bot_id: bot_id.clone(),
                }),
                Source::Discord { bot_id, .. } => Some(NativeSource::Discord {
                    bot_id: bot_id.clone(),
                }),
                Source::Whatsapp {
                    app_id,
                    waba_id,
                    phone_number_id,
                    ..
                } => Some(NativeSource::Whatsapp {
                    app_id: app_id.clone(),
                    waba_id: waba_id.clone(),
                    phone_number_id: phone_number_id.clone(),
                }),
                _ => None,
            });
        match &native {
            Some(NativeSource::Telegram { bot_id }) => {
                activity.source = cmaeg_core::activity::SourceKind::Telegram {
                    platform: cmaeg_core::channel::Platform::Telegram,
                    bot_id: bot_id.clone(),
                };
                activity.conversation.id = actor.into();
            }
            Some(NativeSource::Discord { bot_id }) => {
                activity.source = cmaeg_core::activity::SourceKind::Discord {
                    bot_id: bot_id.clone(),
                };
                activity.conversation.id = actor.into();
            }
            Some(NativeSource::Whatsapp {
                app_id,
                waba_id,
                phone_number_id,
            }) => {
                activity.source = cmaeg_core::activity::SourceKind::Whatsapp {
                    app_id: app_id.clone(),
                    waba_id: waba_id.clone(),
                    phone_number_id: phone_number_id.clone(),
                };
                activity.actor.id = actor.into();
                activity.conversation.id = actor.into();
            }
            None => {}
        }
        let telegram_admission = self.state.telegram_effects_issuance_enabled
            && matches!(native.as_ref(), Some(NativeSource::Telegram { .. }));
        let conversation_id = ConversationId::derive(&self.state.keys, &self.id, 1, actor);
        let event = EventRefPayload {
            telegram_admission: telegram_admission.then(|| {
                cmaeg_core::seal::TelegramAdmissionProof {
                    actor_id: actor.into(),
                    actor_kind: activity.actor.kind,
                    conversation_kind: activity.conversation.kind,
                    mentioned: Some(true),
                    replied_to_bot: Some(false),
                }
            }),
            v: 1,
            activity_sha256: Some(cmaeg_core::seal::event_ref::activity_digest(&activity)),
            channel_id: self.id.clone(),
            record_id: self.definition.record_id.clone(),
            event_id: activity.event_id.clone(),
            partition_key: actor.into(),
            conversation_id: conversation_id.clone(),
            expires_at: now() + 600,
            reply: match native {
                Some(NativeSource::Telegram { bot_id }) => ReplyAddress::Telegram {
                    bot_id,
                    chat_id: actor.into(),
                    message_id: "5".into(),
                    thread_id: None,
                },
                Some(NativeSource::Discord { bot_id }) => ReplyAddress::Discord {
                    bot_id,
                    channel_id: actor.into(),
                    message_id: "700".into(),
                    thread_id: None,
                },
                Some(NativeSource::Whatsapp {
                    app_id,
                    waba_id,
                    phone_number_id,
                }) => ReplyAddress::Whatsapp {
                    app_id,
                    waba_id,
                    phone_number_id,
                    recipient_id: actor.into(),
                    message_id: "wamid.original".into(),
                },
                None => ReplyAddress::NyxidRelay {
                    thread_reply: threaded,
                    message_id: name.into(),
                    route_id: format!("route-{actor}"),
                    platform: if threaded {
                        cmaeg_core::channel::Platform::Lark
                    } else {
                        cmaeg_core::channel::Platform::Telegram
                    },
                    reply_token: cmaeg_core::channel::Secret::new("fixture-unused-reply-token"),
                    reply_token_expires_at: now() + 600,
                },
            },
            consumer,
            lineage: None,
        };
        let event_ref = EventRef::seal(&event, &self.state.keys).unwrap();
        let mut stream = self
            .state
            .providers
            .start_turn(
                &self.definition.provider.slug,
                TurnRequest {
                    nyxid_trace: false,
                    timing: None,
                    conversation_id: conversation_id.clone(),
                    input: vec![],
                    metadata: Default::default(),
                    idempotency_key: IdempotencyKey::from_event(&event.event_id),
                    agent_key: credential,
                    event_context: Some(EventContextSnapshot {
                        schema_version: 1,
                        channel_id: self.id.clone(),
                        conversation_id,
                        activity,
                        event_ref: event_ref.clone(),
                    }),
                },
            )
            .await
            .unwrap();
        (&mut stream.accepted).await.unwrap();
        while stream.events.recv().await.is_some() {}
        event_ref
    }
    pub async fn target(&self, event: &EventRef) -> String {
        let response = self
            .client
            .post(format!(
                "{}/v1/events/{}/reply-target",
                self.base,
                event.as_str()
            ))
            .bearer_auth(&self.key)
            .json(&json!({}))
            .send()
            .await
            .unwrap();
        let status = response.status();
        let value: Value = response.json().await.unwrap();
        assert_eq!(status, 200, "{value}");
        assert!(value["expires_at"].as_i64().unwrap() <= now() + 86400);
        value["reply_target_ref"].as_str().unwrap().into()
    }
    pub fn send(&self, target: &str, id: &str, text: &str) -> reqwest::RequestBuilder {
        self.client
            .post(format!("{}/v1/reply-targets/{target}/messages", self.base))
            .bearer_auth(&self.key)
            .header("idempotency-key", id)
            .json(&json!({"text":text}))
    }
    pub async fn update(&mut self) {
        self.definition.version += 1;
        self.records
            .put(
                &self.definition.record_id,
                &self.definition.owner.subject,
                &seal_blob(
                    &self.state.keys,
                    self.definition.to_blob_json().to_string().as_bytes(),
                )
                .unwrap(),
                Some(self.definition.version - 1),
            )
            .await
            .unwrap();
    }
}
fn engine(
    receiver: mpsc::Receiver<cmaeg::delivery::intents::DeliveryCommand>,
    state: &ToolsState,
    relay: &FakeReply,
    native: Option<cmaeg::telegram::TelegramClient>,
    media_effects: bool,
) -> DeliveryEngine {
    let mut transport = RelayClient::new(&relay.base, Duration::from_millis(200)).unwrap();
    if let Some(native) = native {
        transport = transport
            .with_telegram(native)
            .with_telegram_effects(state.telegram_effects_issuance_enabled)
            .with_telegram_media_effects(media_effects);
    }
    DeliveryEngine::start(
        receiver,
        state.keys.clone(),
        transport,
        DeliveryConfig::default(),
        state.tracker.clone(),
        CustomDelivery::new(state.public_origin.clone(), Arc::default()),
    )
}
pub async fn receipt(response: reqwest::Response) -> Value {
    let status = response.status();
    let value: Value = response.json().await.unwrap();
    assert_eq!(status, 200, "{value}");
    value
}
pub fn now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}
async fn serve(router: axum::Router) -> (String, JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    (
        base,
        tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        }),
    )
}
