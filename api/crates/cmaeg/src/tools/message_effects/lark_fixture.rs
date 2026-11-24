//! Local provider, NyxID proxy, record store and actual delivery engine fixture.
use super::*;
use crate::{
    channels::{definition::fixture, resolver::ChannelResolver},
    delivery::{
        custom::CustomDelivery,
        engine::DeliveryEngine,
        intents::{DeliveryCommand, ReplySource},
        relay_client::RelayClient,
        router::ShardedScheduler,
    },
    interactions::InteractionStore,
    records::RecordClient,
    turns::{routed::adapter_test_server::HttpServer, TurnTracker},
    DeliveryConfig,
};
use axum::{
    body::Bytes,
    extract::{Request, State},
    http::StatusCode,
    response::IntoResponse,
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use cmaeg_core::{
    activity::Activity,
    channel::{ChannelDefinition, Platform},
    seal::{
        event_ref::activity_digest, seal_blob, ChannelPointer, EventRef, EventRefPayload,
        MasterKeyRing, ReplyAddress,
    },
};
use serde_json::{json, Value};
use std::{
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Mutex,
    },
    time::Duration,
};
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
};

#[derive(Clone)]
pub struct Memory {
    pub requests: Arc<Mutex<Vec<(String, String, Value)>>>,
    pub overrides: Arc<Mutex<std::collections::BTreeMap<String, (u16, Value)>>>,
    pub fail_after_write: Arc<AtomicBool>,
    pub write_delay_ms: Arc<AtomicU64>,
    pub records_fail: Arc<AtomicBool>,
    pub platform: Platform,
}
async fn handle(State(memory): State<Memory>, request: Request) -> axum::response::Response {
    let (parts, body) = request.into_parts();
    let path = parts.uri.path_and_query().unwrap().as_str().to_owned();
    let method = parts.method.to_string();
    let body = axum::body::to_bytes(body, 64 * 1024).await.unwrap();
    let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    assert_eq!(
        parts.headers.get("authorization").unwrap(),
        "Bearer lark-channel-fixture"
    );
    memory
        .requests
        .lock()
        .unwrap()
        .push((method.clone(), path.clone(), body.clone()));
    if method != "GET" && path.contains("/proxy/") {
        tokio::time::sleep(Duration::from_millis(
            memory.write_delay_ms.load(Ordering::SeqCst),
        ))
        .await;
    }
    let replacement = memory
        .overrides
        .lock()
        .unwrap()
        .get(&format!("{method} {path}"))
        .cloned();
    if method != "GET" && memory.fail_after_write.load(Ordering::SeqCst) {
        memory.records_fail.store(true, Ordering::SeqCst);
    }
    if let Some((status, body)) = replacement {
        return (StatusCode::from_u16(status).unwrap(), Json(body)).into_response();
    }
    let (slug, origin) =
        cmaeg_core::channel::lark_resources::lark_resource_service(memory.platform).unwrap();
    let value = if path == "/api/v1/user-services" {
        json!({"services":[{"id":"connection-one","slug":slug,"catalog_service_id":"catalog-one","endpoint_id":"endpoint-one","is_active":true,"auth_method":"token_exchange"}]})
    } else if path == "/api/v1/endpoints" {
        json!({"endpoints":[{"id":"endpoint-one","catalog_service_id":"catalog-one","url":origin}]})
    } else if path == "/api/v1/services/catalog-one" {
        json!({"id":"catalog-one","slug":slug,"base_url":origin,"auth_method":"token_exchange","is_active":true})
    } else if path == "/api/v1/channel-relay/reply" {
        json!({"message_id":"outbound-one","platform_message_id":"om_sent"})
    } else if path.ends_with("/bot/v3/info?_nyxid_via=connection-one") {
        json!({"code":0,"bot":{"open_id":"ou_source"}})
    } else if method == "GET" && path == MESSAGE {
        json!({"code":0,"data":{"items":[{"message_id":"om_sent","chat_id":"oc_chat","sender":{"sender_type":"app","id_type":"app_id","id":"cli_source"}}]}})
    } else if method == "DELETE" && path == MESSAGE {
        json!({"code":0})
    } else if method == "POST" && path == REACTIONS {
        let emoji = body["reaction_type"]["emoji_type"].as_str().unwrap();
        reaction(&format!("reaction_{emoji}"), emoji)
    } else if method == "DELETE" && path.contains("/reactions/reaction_") {
        let emoji = path
            .split("/reactions/reaction_")
            .nth(1)
            .unwrap()
            .split('?')
            .next()
            .unwrap();
        reaction(&format!("reaction_{emoji}"), emoji)
    } else {
        return (StatusCode::NOT_FOUND, Bytes::new()).into_response();
    };
    Json(value).into_response()
}
fn reaction(id: &str, emoji: &str) -> Value {
    json!({"code":0,"data":{"reaction_id":id,"operator":{"operator_id":"cli_source","operator_type":"app"},"reaction_type":{"emoji_type":emoji}}})
}
pub const MESSAGE: &str =
    "/api/v1/proxy/catalog-one/open-apis/im/v1/messages/om_sent?_nyxid_via=connection-one";
pub const REACTIONS: &str = "/api/v1/proxy/catalog-one/open-apis/im/v1/messages/om_sent/reactions?_nyxid_via=connection-one";
pub struct App {
    pub backend: super::super::fake_cma::FakeCma,
    pub state: ToolsState,
    pub definition: ChannelDefinition,
    pub event: EventRefPayload,
    pub activity: Activity,
    pub provider: HttpServer,
    pub memory: Memory,
    pub base: String,
    pub key: String,
    engine: Option<DeliveryEngine>,
    task: JoinHandle<()>,
}
impl Drop for App {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl App {
    pub async fn new(platform: Platform) -> Self {
        let backend = super::super::fake_cma::FakeCma::start(false).await;
        let provider = HttpServer::start().await;
        let memory = Memory {
            requests: Arc::default(),
            overrides: Arc::default(),
            fail_after_write: Arc::default(),
            write_delay_ms: Arc::default(),
            records_fail: backend.fail_put.clone(),
            platform,
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let router = Router::new().fallback(handle).with_state(memory.clone());
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let records = RecordClient::new(&backend.base_url, "cma-service").unwrap();
        let keys =
            Arc::new(MasterKeyRing::parse(&format!("1:{}", STANDARD.encode([3; 32]))).unwrap());
        let key = "nyxid_ag_lark-effects-agent".to_owned();
        let mut definition = fixture::with_agent_key(&key);
        definition.record_id = uuid::Uuid::new_v4().to_string().parse().unwrap();
        definition.secrets.channel_key =
            Some(cmaeg_core::channel::Secret::new("lark-channel-fixture"));
        definition.sources = vec![serde_json::from_value(json!({"type":"nyxid_relay","issuer":"https://nyx.example","jwks_url":"https://nyx.example/jwks","key_id":"key-one","platform":platform,"route_ids":["route-one"],"bot_id":"ou_source","admission":{"type":"all"},"lark_message_effects":{"connection":{"user_service_id":"connection-one","catalog_service_id":"catalog-one","app_id":"cli_source"},"delete":true,"reaction":true}})).unwrap()];
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
        activity.source = cmaeg_core::activity::SourceKind::NyxidRelay {
            platform,
            route_id: "route-one".into(),
        };
        activity.conversation.id = "oc_chat".into();
        let event = EventRefPayload {
            v: 1,
            telegram_admission: None,
            activity_sha256: Some(activity_digest(&activity)),
            channel_id: id,
            record_id: definition.record_id.clone(),
            event_id: activity.event_id.clone(),
            partition_key: "lark:oc_chat".into(),
            conversation_id: "conv_00000000000000000000000000000001".parse().unwrap(),
            expires_at: now() + 600,
            reply: ReplyAddress::NyxidRelay {
                thread_reply: false,
                message_id: "original-one".into(),
                route_id: "route-one".into(),
                platform,
                reply_token: cmaeg_core::channel::Secret::new("unused"),
                reply_token_expires_at: 0,
            },
            consumer: None,
            lineage: None,
        };
        provider.memory.lock().unwrap().config.event_context = Some(
            json!({"schema_version":1,"channel_id":event.channel_id,"conversation_id":event.conversation_id,"event_ref":EventRef::seal(&event, &keys).unwrap(),"activity":activity}),
        );
        let (sender, _) = mpsc::channel(16);
        let state = ToolsState {
            discord_thread_history: None,
            telegram_topics: None,
            discord_threads: None,
            native_lark_conversation_history: None,
            discord_conversation_history: None,
            complete_replies_enabled: true,
            commonmark_relay: Default::default(),
            telegram_effects_issuance_enabled: false,
            native_telegram_media_controls_enabled: false,
            native_telegram_media_control_edits_enabled: false,
            telegram_effects: None,
            native_media_edits: None,
            native_whatsapp_forms: None,
            native_typing: None,
            native_lark_effects: None,
            lark_effects_issuance_enabled: true,
            lark_effects: Some(Arc::new(
                lark_transport::LarkEffectsClient::new(&base).unwrap(),
            )),
            whatsapp_templates_enabled: false,
            discord_interactions_enabled: false,
            native_discord_media_controls_enabled: false,
            native_discord_media_card_edits_enabled: false,
            native_discord_media_card_replacements_enabled: false,
            native_discord_media_card_sets_enabled: false,
            whatsapp_interactions_enabled: false,
            native_whatsapp_interactions_enabled: false,
            native_whatsapp_media_controls_enabled: false,
            consumer_interactions_enabled: true,
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
            relay_text_edits_enabled: true,
            native_discord_edits_enabled: false,
            discord_effects_issuance_enabled: false,
            discord_effects: None,
            lark_cards_enabled: true,
            resolver: ChannelResolver::new(Arc::new(records), keys.clone(), Default::default()),
            keys,
            providers: provider.client(true, Duration::from_secs(5)),
            delivery: Arc::new(ShardedScheduler::new(sender)),
            routed_replies: None,
            tracker: TurnTracker::new(crate::http::readiness::Readiness::initialized()),
            public_origin: "https://gateway.example".into(),
            public_api_url: "https://gateway.example".into(),
            public_api_aliases: vec![],
            advertise_public_api_url: true,
            media: None,
            downloads: Default::default(),
        };
        let mut app = Self {
            backend,
            state,
            definition,
            event,
            activity,
            provider,
            memory,
            base,
            key,
            engine: None,
            task,
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
        let store = Arc::new(InteractionStore::new(
            records,
            self.state.keys.clone(),
            self.state.providers.clone(),
        ));
        let relay = RelayClient::new(&self.base, Duration::from_secs(2))
            .unwrap()
            .with_complete_replies(true)
            .with_text_edits(true)
            .with_lark_cards(true)
            .with_consumer_interactions(true)
            .with_interactions(store);
        let (sender, receiver) = mpsc::channel(16);
        self.state.delivery = Arc::new(ShardedScheduler::new(sender));
        self.engine = Some(DeliveryEngine::start(
            receiver,
            self.state.keys.clone(),
            relay,
            DeliveryConfig::default(),
            self.state.tracker.clone(),
            CustomDelivery::new(self.state.public_origin.clone(), Arc::default()),
        ));
    }
    pub fn caller(&self) -> AgentCaller {
        AgentCaller {
            key_hash: self.state_key(),
        }
    }
    fn state_key(&self) -> [u8; 32] {
        use sha2::Digest;
        sha2::Sha256::digest(self.key.as_bytes()).into()
    }
    pub fn named_consumer(&mut self) {
        use crate::channels::routing_fixture as routing;
        let routed = routing::definition(routing::routed_with_key(&self.key));
        self.definition.routing = routed.routing;
        self.definition.secrets.consumer_keys = routed.secrets.consumer_keys;
        self.definition.validate().unwrap();
        self.key = routing::agent_key(routing::REVIEWER);
        self.event.consumer = Some(cmaeg_core::ids::ConsumerId::parse(routing::REVIEWER).unwrap());
        self.provider.memory.lock().unwrap().config.event_context = Some(
            json!({"schema_version":1,"channel_id":self.event.channel_id,"conversation_id":self.event.conversation_id,"event_ref":EventRef::seal(&self.event, &self.state.keys).unwrap(),"activity":self.activity}),
        );
        self.replace();
    }
    pub fn replace(&self) {
        self.backend.seed(
            self.definition.record_id.as_str(),
            &self.definition.owner.subject,
            seal_blob(
                &self.state.keys,
                &serde_json::to_vec(&self.definition.to_blob_json()).unwrap(),
            )
            .unwrap(),
        );
    }
    pub async fn send(&self, raw: Value) -> crate::delivery::intents::IntentReceipt {
        let channel = self
            .state
            .resolver
            .resolve_current(self.event.channel_id.as_str())
            .await
            .unwrap();
        let (receipt, received) = oneshot::channel();
        self.state
            .delivery
            .submit(DeliveryCommand::Reply {
                trace: tracing::Span::none(),
                channel,
                event: self.event.clone(),
                message: serde_json::from_value(raw).unwrap(),
                source: ReplySource::Tool,
                receipt,
            })
            .await
            .unwrap();
        received.await.unwrap()
    }
    pub async fn reference(&self) -> MessageRef {
        let receipt = self.send(json!({"text":"hello"})).await;
        assert_eq!(receipt.delivery.kind(), "confirmed");
        let reference = receipt.message_ref.unwrap();
        assert!(reference
            .open(&self.state.keys)
            .unwrap()
            .payload
            .lark_effect
            .is_some());
        reference
    }
    pub fn writes(&self) -> Vec<(String, String, Value)> {
        self.memory
            .requests
            .lock()
            .unwrap()
            .iter()
            .filter(|(method, path, _)| method != "GET" && path.contains("/proxy/"))
            .cloned()
            .collect()
    }
    pub fn override_response(&self, method: &str, path: &str, status: u16, body: Value) {
        self.memory
            .overrides
            .lock()
            .unwrap()
            .insert(format!("{method} {path}"), (status, body));
    }
}
