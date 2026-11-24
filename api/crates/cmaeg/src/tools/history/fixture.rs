use super::*;
use crate::{
    channels::{resolver::ChannelResolver, routing_fixture},
    ingress::{
        launch::LaunchRequest,
        routing_plan::{self, OrdinaryPlan, RoutingOptions},
        routing_store::{Candidate, PlanStore},
    },
    providers::{ProviderClient, ProviderRegistry},
    records::RecordClient,
    tools::fake_cma,
};
use axum::{
    body::{to_bytes, Body},
    extract::State,
    http::{Request, StatusCode},
    response::{IntoResponse, Response},
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use cmaeg_core::seal::EventRefPayload;
use cmaeg_core::{
    activity::{Activity, SourceKind},
    channel::ChannelDefinition,
    seal::{event_ref::activity_digest, seal_blob, ChannelPointer, MasterKeyRing, ReplyAddress},
};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Mutex,
    time::{Duration, Instant},
};
use tokio::{sync::mpsc, task::JoinHandle};

pub const DEFAULT_KEY: &str = "nyx_fixture_history_default";

pub struct Memory {
    pub calls: Vec<(String, String, Value)>,
    pub channels: BTreeMap<String, Value>,
    pub rows: Value,
    pub final_anchor: Option<Value>,
    pub final_channel: Option<Value>,
    pub message: Value,
    pub delay: Option<(String, String, Duration)>,
    pub status: StatusCode,
    pub hook: Option<Box<dyn FnOnce() + Send>>,
    pub hook_path: String,
    pub hook_method: String,
}
pub struct App {
    pub backend: Arc<fake_cma::FakeCma>,
    pub memory: Arc<Mutex<Memory>>,
    pub state: ToolsState,
    pub definition: ChannelDefinition,
    pub event: EventRefPayload,
    pub activity: Activity,
    pub key: String,
    pub base: String,
    pub source_body: Value,
    servers: Vec<JoinHandle<()>>,
}
impl Drop for App {
    fn drop(&mut self) {
        for server in &self.servers {
            server.abort();
        }
    }
}
pub fn thread(id: &str, kind: u8, parent: &str, archived: bool) -> Value {
    json!({"id":id,"type":kind,"guild_id":"100","parent_id":parent,"owner_id":"77","name":"Review",
        "thread_metadata":{"archived":archived,"locked":false,"auto_archive_duration":1440}})
}
async fn serve(router: Router) -> (String, JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    (
        format!("http://{}", listener.local_addr().unwrap()),
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() }),
    )
}
async fn platform(State(memory): State<Arc<Mutex<Memory>>>, request: Request<Body>) -> Response {
    let (parts, body) = request.into_parts();
    assert_eq!(parts.headers["authorization"], "Bot history-fixture");
    assert_eq!(
        parts.method, "GET",
        "history never performs platform writes"
    );
    let bytes = to_bytes(body, 8192).await.unwrap();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    let method = parts.method.as_str();
    let (response, delay) = {
        let mut memory = memory.lock().unwrap();
        let delay = memory
            .delay
            .as_ref()
            .filter(|(verb, path, _)| verb == method && path == parts.uri.path())
            .map(|(_, _, delay)| *delay);
        (
            platform_response(&mut memory, method, &parts.uri, body),
            delay,
        )
    };
    if let Some(delay) = delay {
        tokio::time::sleep(delay).await;
    }
    response
}
fn platform_response(
    memory: &mut Memory,
    method: &str,
    uri: &axum::http::Uri,
    body: Value,
) -> Response {
    let path = uri.path();
    memory
        .calls
        .push((method.into(), uri.to_string(), body.clone()));
    if path == memory.hook_path && (memory.hook_method.is_empty() || memory.hook_method == method) {
        if let Some(hook) = memory.hook.take() {
            hook();
        }
    }
    if path == "/api/v10/users/@me" {
        return Json(json!({"id":"77","bot":true})).into_response();
    }
    if method == "GET" {
        if path.ends_with("/messages") {
            let query: BTreeMap<String, String> =
                url::form_urlencoded::parse(uri.query().unwrap().as_bytes())
                    .into_owned()
                    .collect();
            assert_eq!(query.len(), 2);
            assert_eq!(query["limit"], "26");
            let before = query["before"].parse::<u64>().unwrap();
            let rows = memory
                .rows
                .as_array()
                .unwrap()
                .iter()
                .filter(|row| {
                    row["id"]
                        .as_str()
                        .and_then(|id| id.parse::<u64>().ok())
                        .is_none_or(|id| id < before)
                })
                .take(26)
                .cloned()
                .collect::<Vec<_>>();
            if let Some(anchor) = memory.final_anchor.take() {
                memory.message = anchor;
            }
            if let Some(channel) = memory.final_channel.take() {
                memory
                    .channels
                    .insert(channel["id"].as_str().unwrap().into(), channel);
            }
            return (memory.status, Json(rows)).into_response();
        }
        if path.ends_with("/messages/700") {
            return Json(memory.message.clone()).into_response();
        }
        if let Some(id) = path.strip_prefix("/api/v10/channels/") {
            return memory
                .channels
                .get(id)
                .cloned()
                .map(Json)
                .map(IntoResponse::into_response)
                .unwrap_or_else(|| StatusCode::NOT_FOUND.into_response());
        }
    }
    StatusCode::NOT_FOUND.into_response()
}
impl App {
    pub async fn new(origin_type: u8, named: bool) -> Self {
        Self::with_setup(origin_type, named, |_| {}).await
    }
    pub async fn with_setup(origin_type: u8, named: bool, setup: impl FnOnce(&mut App)) -> Self {
        let thread_kind = matches!(origin_type, 10..=12).then_some(origin_type);
        let backend = Arc::new(fake_cma::FakeCma::start(false).await);
        let records = RecordClient::new(&backend.base_url, "cma-service").unwrap();
        let keys =
            Arc::new(MasterKeyRing::parse(&format!("1:{}", STANDARD.encode([3; 32]))).unwrap());
        let mut value = routing_fixture::routed_with_key(DEFAULT_KEY);
        value["sources"] = json!([{"type":"discord","bot_id":"77","intents":37377,"admission":{"type":"all"},"conversation_history":true}]);
        value["secrets"]["discord_bot_token"] = "history-fixture".into();
        value["routing"]["default_targets"] = json!([if named {
            routing_fixture::REVIEWER
        } else {
            "default"
        }]);
        let mut definition = routing_fixture::definition(value);
        definition.record_id = uuid::Uuid::new_v4().to_string().parse().unwrap();
        definition.validate().unwrap();
        let key = if named {
            routing_fixture::agent_key(routing_fixture::REVIEWER)
        } else {
            DEFAULT_KEY.into()
        };
        let pointer = ChannelPointer {
            v: 1,
            record_id: definition.record_id.clone(),
            record_url: records
                .record_url(definition.record_id.as_str())
                .unwrap()
                .to_string(),
        };
        let channel_id = pointer.seal_id(&keys).unwrap();
        let origin = if thread_kind.is_some() { "99" } else { "98" };
        let mut activity: Activity = serde_json::from_str(include_str!(
            "../../../../cmaeg-core/tests/fixtures/activity.json"
        ))
        .unwrap();
        activity.source = SourceKind::Discord {
            bot_id: "77".into(),
        };
        activity.actor.id = "51".into();
        activity.conversation.id = origin.into();
        activity.conversation.kind = if origin_type == 1 {
            cmaeg_core::activity::ConversationKind::Private
        } else {
            cmaeg_core::activity::ConversationKind::Group
        };
        activity.conversation.thread_id = thread_kind.map(|_| "99".into());
        activity.event_id = "discord:77:700".parse().unwrap();
        let partition_key = activity.partition_key(&definition.partition);
        let conversation_id = cmaeg_core::ids::ConversationId::derive(
            &keys,
            &channel_id,
            definition.partition.version,
            &partition_key,
        );
        let event = EventRefPayload {
            v: 1,
            telegram_admission: None,
            activity_sha256: Some(activity_digest(&activity)),
            channel_id,
            record_id: definition.record_id.clone(),
            event_id: activity.event_id.clone(),
            partition_key,
            conversation_id,
            expires_at: now() + 600,
            reply: ReplyAddress::Discord {
                bot_id: "77".into(),
                channel_id: origin.into(),
                thread_id: thread_kind.map(|_| "99".into()),
                message_id: "700".into(),
            },
            consumer: None,
            lineage: None,
        };
        let origin_channel = thread_kind.map_or_else(
            || json!({"id":"98","type":origin_type,"guild_id":if origin_type == 1 {None} else {Some("100")}}),
            |kind| thread("99", kind, "98", false),
        );
        let memory = Arc::new(Mutex::new(Memory {
            calls: vec![],
            channels: BTreeMap::from([(origin.into(), origin_channel)]),
            rows: json!([]),
            final_anchor: None,
            final_channel: None,
            message: json!({"id":"700","channel_id":origin,"type":0,"author":{"id":"51","bot":false}}),
            delay: None,
            status: StatusCode::OK,
            hook: None,
            hook_path: String::new(),
            hook_method: String::new(),
        }));
        let (platform_base, server) =
            serve(Router::new().fallback(platform).with_state(memory.clone())).await;
        let registry = ProviderRegistry::parse("[]").unwrap();
        let (sender, _) = mpsc::channel(1);
        let state = ToolsState {
            native_whatsapp_forms: None,
            native_typing: None,
            native_media_edits: None,
            telegram_topics: None,
            discord_threads: None,
            native_lark_conversation_history: None,
            discord_conversation_history: Some(Arc::new(DiscordConversationHistory::new(
                DiscordRest::fixture(&format!("{platform_base}/api/v10/")),
            ))),
            discord_thread_history: None,
            complete_replies_enabled: false,
            commonmark_relay: Default::default(),
            telegram_effects_issuance_enabled: false,
            native_telegram_media_controls_enabled: false,
            native_telegram_media_control_edits_enabled: false,
            telegram_effects: None,
            lark_effects_issuance_enabled: false,
            lark_effects: None,
            native_lark_effects: None,
            whatsapp_reactions: None,
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
            native_whatsapp_templates_enabled: false,
            native_whatsapp_groups_enabled: false,
            relay_text_edits_enabled: false,
            native_discord_edits_enabled: false,
            discord_effects_issuance_enabled: false,
            discord_effects: None,
            lark_cards_enabled: false,
            resolver: ChannelResolver::new(Arc::new(records), keys.clone(), Default::default()),
            keys,
            providers: Arc::new(
                ProviderClient::new(registry, Duration::from_secs(1), Duration::from_secs(1), 4)
                    .unwrap(),
            ),
            delivery: Arc::new(crate::delivery::router::ShardedScheduler::new(sender)),
            routed_replies: None,
            tracker: crate::turns::TurnTracker::new(
                crate::http::readiness::Readiness::initialized(),
            ),
            public_origin: "https://gateway.example".into(),
            public_api_url: "https://gateway.example".into(),
            public_api_aliases: vec![],
            advertise_public_api_url: true,
            media: None,
            downloads: Default::default(),
        };
        let source_body = json!({"channel":definition.to_public_json(),"dispatch":{"id":"700","channel_id":origin,"guild_id":if origin_type == 1 {None} else {Some("100")},"type":0,"author":{"id":"51","bot":false}},
            "native_channel":{"status":"verified","channel_id":origin,"channel_type":origin_type,"guild_id":if origin_type == 1 {None} else {Some("100")},"parent_id":thread_kind.map(|_|"98")}});
        let mut app = Self {
            backend,
            memory,
            state,
            definition,
            event,
            activity,
            key,
            source_body,
            base: String::new(),
            servers: vec![server],
        };
        setup(&mut app);
        app.save();
        app.admit().await;
        app.mount().await;
        app
    }
    pub fn save(&self) {
        self.seed(&self.definition);
    }
    pub fn seed(&self, definition: &ChannelDefinition) {
        self.backend.seed(
            definition.record_id.as_str(),
            &definition.owner.subject,
            seal_blob(
                &self.state.keys,
                &serde_json::to_vec(&definition.to_blob_json()).unwrap(),
            )
            .unwrap(),
        );
    }
    pub fn reference(&self) -> EventRef {
        EventRef::seal(&self.event, &self.state.keys).unwrap()
    }
    async fn admit(&mut self) {
        let channel = self
            .state
            .resolver
            .resolve_current(self.event.channel_id.as_str())
            .await
            .unwrap();
        let request = LaunchRequest {
            channel,
            activity: self.activity.clone(),
            conversation_id: self.event.conversation_id.clone(),
            event_ref: self.reference(),
            idempotency_key: cmaeg_core::ids::IdempotencyKey::from_event(&self.activity.event_id),
            received_at: Instant::now(),
            trace: tracing::Span::none(),
            timing: None,
            consumer: None,
        };
        let OrdinaryPlan::Routed(plan) = routing_plan::plan(
            &request,
            &self.state.keys,
            now(),
            RoutingOptions {
                enabled: true,
                attributes_signed: true,
                non_human_providers: &BTreeSet::new(),
            },
        )
        .unwrap() else {
            panic!()
        };
        let candidate = Candidate::prepare(
            &request,
            &plan,
            &serde_json::to_vec(&self.source_body).unwrap(),
            &self.state.keys,
            now(),
        )
        .unwrap();
        PlanStore::new(self.state.resolver.record_client(), self.state.keys.clone())
            .admit(&candidate)
            .await
            .unwrap();
        self.activity = plan.activity;
        self.event = plan.targets[0].event.clone();
    }
    pub async fn mount(&mut self) {
        let (base, server) = serve(
            crate::http::tools::router(self.state.clone())
                .merge(crate::mcp::router(self.state.clone())),
        )
        .await;
        self.base = base;
        self.servers.push(server);
    }
    pub async fn read(&self, cursor: Option<&str>) -> reqwest::Response {
        let request = reqwest::Client::new()
            .get(format!(
                "{}/v1/events/{}/history",
                self.base,
                self.reference().as_str()
            ))
            .bearer_auth(&self.key);
        let request = if let Some(cursor) = cursor {
            request.query(&[("cursor", cursor)])
        } else {
            request
        };
        request.send().await.unwrap()
    }
    pub fn set_rows(&self, count: usize) {
        let channel = &self.activity.conversation.id;
        self.memory.lock().unwrap().rows = json!((0..count)
            .map(|i| row(699 - i as u64, channel))
            .collect::<Vec<_>>());
    }
    pub fn writes(&self) -> Vec<(String, String, Value)> {
        self.memory
            .lock()
            .unwrap()
            .calls
            .iter()
            .filter(|(method, _, _)| method != "GET")
            .cloned()
            .collect()
    }
    pub fn change_on_read(&self, path: &str, change: impl FnOnce(&mut ChannelDefinition)) {
        let mut definition = self.definition.clone();
        change(&mut definition);
        let backend = self.backend.clone();
        let keys = self.state.keys.clone();
        let mut memory = self.memory.lock().unwrap();
        memory.hook_path = path.into();
        memory.hook = Some(Box::new(move || {
            backend.seed(
                definition.record_id.as_str(),
                &definition.owner.subject,
                seal_blob(
                    &keys,
                    &serde_json::to_vec(&definition.to_blob_json()).unwrap(),
                )
                .unwrap(),
            );
        }));
    }
}

pub fn row(id: u64, channel: &str) -> Value {
    json!({"id":id.to_string(),"channel_id":channel,"type":0,
        "author":{"id":"51","username":"Earlier author","bot":false},
        "content":"Earlier context","timestamp":"2026-10-28T00:00:00Z","attachments":[]})
}
