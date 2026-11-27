use crate::{
    auth::AgentCaller,
    channels::{definition::fixture, resolver::ChannelResolver},
    delivery::{
        custom::CustomDelivery, engine::DeliveryEngine, relay_client::RelayClient,
        router::ShardedScheduler,
    },
    discord::rest_fixture::Fixture as Discord,
    providers::EventContextSnapshot,
    records::RecordClient,
    tools::{fake_cma::FakeCma, ToolsState},
    turns::routed::adapter_test_server::HttpServer,
    whatsapp::{self, groups::tests as group},
    DeliveryConfig,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use cmaeg_core::{
    activity::{Activity, ConversationKind, SourceKind},
    channel::{AdmissionPolicy, ChannelDefinition, Secret, Source},
    seal::{
        event_ref::activity_digest, seal_blob, ChannelPointer, EventRef, EventRefPayload,
        MasterKeyRing, ReplyAddress, ReplyTargetRef,
    },
};
use serde_json::json;
use std::{sync::Arc, time::Duration};

#[path = "native_state.rs"]
pub(crate) mod native_state;

#[derive(Clone, Copy, Debug)]
pub(super) enum Variant {
    Discord,
    DiscordThread,
    Whatsapp,
    WhatsappGroup,
}
pub(super) const VARIANTS: [Variant; 4] = [
    Variant::Discord,
    Variant::DiscordThread,
    Variant::Whatsapp,
    Variant::WhatsappGroup,
];

pub(super) struct App {
    pub backend: FakeCma,
    pub discord: Discord,
    pub graph: Arc<group::Graph>,
    pub state: ToolsState,
    pub caller: AgentCaller,
    pub event: EventRef,
    pub expiry: i64,
    definition: ChannelDefinition,
    _provider: HttpServer,
    engine: Option<DeliveryEngine>,
}

impl App {
    pub async fn new(variant: Variant, lost_response: bool) -> Self {
        let backend = FakeCma::start(false).await;
        let discord = Discord::new().await;
        *discord.memory.broken.lock().unwrap() = lost_response;
        let provider = HttpServer::start().await;
        let keys =
            Arc::new(MasterKeyRing::parse(&format!("1:{}", STANDARD.encode([3; 32]))).unwrap());
        let key = format!("nyxid_ag_{}", uuid::Uuid::new_v4().simple());
        let mut definition = fixture::with_agent_key(&key);
        definition.partition.key = cmaeg_core::channel::PartitionKey::ConversationAndSender;
        definition.record_id = uuid::Uuid::new_v4().to_string().parse().unwrap();
        let mut activity: Activity = serde_json::from_str(include_str!(
            "../../../../cmaeg-core/tests/fixtures/activity.json"
        ))
        .unwrap();
        activity.actor.kind = cmaeg_core::activity::ActorKind::Human;
        activity.conversation.thread_id = None;
        let reply = match variant {
            Variant::Discord | Variant::DiscordThread => {
                definition.sources = vec![Source::Discord {
                    bot_id: "77".into(),
                    intents: 37377,
                    thread_history: false,
                    thread_lifecycle: false,
                    conversation_history: false,
                    admission: AdmissionPolicy::All {},
                }];
                definition.secrets.discord_bot_token = Some(Secret::new("native-local-fixture"));
                activity.source = SourceKind::Discord {
                    bot_id: "77".into(),
                };
                activity.conversation.id = "99".into();
                let thread_id = matches!(variant, Variant::DiscordThread).then(|| "101".to_owned());
                activity.conversation.thread_id = thread_id.clone();
                if let Some(thread) = &thread_id {
                    activity.conversation.id = thread.clone();
                }
                if thread_id.is_some() {
                    discord.respond(
                        reqwest::StatusCode::OK,
                        json!({"id":"800", "channel_id":"101", "author":{"id":"77"}}),
                    );
                }
                ReplyAddress::Discord {
                    bot_id: "77".into(),
                    channel_id: activity.conversation.id.clone(),
                    message_id: "700".into(),
                    thread_id,
                }
            }
            Variant::Whatsapp | Variant::WhatsappGroup => {
                let is_group = matches!(variant, Variant::WhatsappGroup);
                definition.sources = vec![if is_group {
                    group::source_group()
                } else {
                    whatsapp::tests::source()
                }];
                definition.secrets.whatsapp = Some(whatsapp::tests::credentials());
                activity.source = SourceKind::Whatsapp {
                    app_id: "12345".into(),
                    waba_id: "23456".into(),
                    phone_number_id: "34567".into(),
                };
                activity.actor.id = "447700900123".into();
                activity.conversation.id = if is_group {
                    group::GROUP
                } else {
                    "447700900123"
                }
                .into();
                activity.conversation.kind = if is_group {
                    ConversationKind::Group
                } else {
                    ConversationKind::Private
                };
                if is_group {
                    ReplyAddress::WhatsappGroup(group::reply())
                } else {
                    ReplyAddress::Whatsapp {
                        app_id: "12345".into(),
                        waba_id: "23456".into(),
                        phone_number_id: "34567".into(),
                        recipient_id: "447700900123".into(),
                        message_id: "wamid.original".into(),
                    }
                }
            }
        };
        let mut replies = if matches!(variant, Variant::WhatsappGroup) {
            group::proofs()
        } else {
            vec![]
        };
        replies.push(if lost_response {Err(crate::net::exchange::TransportError::Unavailable)} else {
            group::answer(json!({"messaging_product":"whatsapp", "contacts":[{"wa_id":"447700900123"}], "messages":[{"id":"wamid.confirmed", "group_id":group::GROUP}]}))
        });
        let graph = group::Graph::new(replies);
        definition.validate().unwrap();
        let records = RecordClient::new(&backend.base_url, "cma-service").unwrap();
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
        backend.seed(
            definition.record_id.as_str(),
            &definition.owner.subject,
            seal_blob(&keys, definition.to_blob_json().to_string().as_bytes()).unwrap(),
        );
        let state = native_state::state(records, keys, &provider);
        let channel = state.resolver.resolve_current(id.as_str()).await.unwrap();
        let caller = AgentCaller {
            key_hash: *channel.consumer_key_hash(None).unwrap(),
        };
        let expiry = super::now() + 3;
        let payload = EventRefPayload {
            v: 1,
            telegram_admission: None,
            activity_sha256: Some(activity_digest(&activity)),
            channel_id: id.clone(),
            record_id: definition.record_id.clone(),
            event_id: activity.event_id.clone(),
            conversation_id: cmaeg_core::ids::ConversationId::derive(
                &state.keys,
                &id,
                1,
                "native-original",
            ),
            partition_key: "native-original".into(),
            expires_at: expiry,
            reply,
            consumer: None,
            lineage: None,
        };
        let event = EventRef::seal(&payload, &state.keys).unwrap();
        provider.memory.lock().unwrap().config.event_context = Some(
            serde_json::to_value(EventContextSnapshot {
                schema_version: 1,
                channel_id: id,
                conversation_id: payload.conversation_id,
                activity,
                event_ref: event.clone(),
            })
            .unwrap(),
        );
        let mut app = Self {
            backend,
            discord,
            graph,
            state,
            caller,
            event,
            expiry,
            definition,
            _provider: provider,
            engine: None,
        };
        app.restart();
        app
    }

    pub fn restart(&mut self) {
        self.engine.take();
        let records = self.state.resolver.record_client();
        self.state.resolver = ChannelResolver::new(
            Arc::new(records.clone()),
            self.state.keys.clone(),
            Default::default(),
        );
        let mut relay = RelayClient::new("http://127.0.0.1:1", Duration::from_secs(1))
            .unwrap()
            .with_native_whatsapp_groups(true)
            .with_whatsapp(
                whatsapp::Client::production()
                    .unwrap()
                    .with_transport(self.graph.clone()),
                Arc::new(whatsapp::receipts::Store::new(
                    records,
                    self.state.keys.clone(),
                )),
            );
        relay.discord = self.discord.api.clone();
        let (sender, receiver) = tokio::sync::mpsc::channel(16);
        self.state.delivery = Arc::new(ShardedScheduler::new(sender));
        self.engine = Some(DeliveryEngine::start(
            receiver,
            self.state.keys.clone(),
            relay,
            DeliveryConfig::default(),
            self.state.tracker.clone(),
            CustomDelivery::new("https://gateway.example".into(), Arc::default()),
        ));
    }

    pub fn side_messages(&self) -> crate::delivery::side_messages::SideMessageDelivery {
        self.engine
            .as_ref()
            .unwrap()
            .side_messages_if(true)
            .unwrap()
    }

    pub async fn target(&self) -> ReplyTargetRef {
        self.state
            .create_reply_target(self.caller, &self.event)
            .await
            .unwrap()
            .reply_target_ref
    }
    pub async fn send(
        &self,
        target: &ReplyTargetRef,
        key: &str,
    ) -> crate::delivery::intents::IntentReceipt {
        self.state
            .reply_to_target(
                self.caller,
                target,
                key,
                serde_json::from_value(json!({"text":"approved result"})).unwrap(),
            )
            .await
            .unwrap()
    }
    pub fn revoke(&self) {
        let mut definition = self.definition.clone();
        definition.sources.clear();
        self.backend.seed(
            definition.record_id.as_str(),
            &definition.owner.subject,
            seal_blob(
                &self.state.keys,
                definition.to_blob_json().to_string().as_bytes(),
            )
            .unwrap(),
        );
    }
    pub fn sends(&self) -> usize {
        self.discord
            .memory
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|call| call.method == "POST")
            .count()
            + self
                .graph
                .calls
                .lock()
                .unwrap()
                .iter()
                .filter(|(method, _, _)| *method == reqwest::Method::POST)
                .count()
    }
}
