//! Real authenticated REST/MCP and Discord HTTP reads; never external sends.
use super::{fake_cma, DiscordThreadHistory, ToolsState};
use crate::{
    channels::{definition::fixture, resolver::ChannelResolver},
    providers::{ProviderClient, ProviderRegistry},
    records::RecordClient,
};
use axum::{
    body::Body,
    extract::State,
    http::{Request, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use cmaeg_core::{
    activity::{Activity, SourceKind},
    channel::{AdmissionPolicy, ChannelDefinition, Source},
    seal::{
        event_ref::activity_digest, seal_blob, ChannelPointer, EventRef, EventRefPayload,
        MasterKeyRing, ReplyAddress,
    },
};
use serde_json::{json, Value};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{sync::mpsc, task::JoinHandle};

struct Memory {
    calls: Vec<(String, String)>,
    channel: Value,
    messages: Value,
    snapshot: Value,
    status: StatusCode,
    anchor_status: StatusCode,
    history_release_at: Option<time::OffsetDateTime>,
    on_history: Option<Box<dyn FnOnce() + Send>>,
}
struct App {
    backend: Arc<fake_cma::FakeCma>,
    memory: Arc<Mutex<Memory>>,
    state: ToolsState,
    definition: ChannelDefinition,
    event: EventRefPayload,
    activity: Activity,
    key: String,
    base: String,
    servers: Vec<JoinHandle<()>>,
}
impl Drop for App {
    fn drop(&mut self) {
        for server in &self.servers {
            server.abort();
        }
    }
}
async fn serve(router: Router) -> (String, JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    (
        base,
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() }),
    )
}
fn history_message(id: &str) -> Value {
    json!({"id":id,"channel_id":"99","type":0,"author":{"id":"51","username":"reader","bot":false},"content":format!("message {id}"),"timestamp":"2026-10-27T01:00:00Z","edited_timestamp":null,"attachments":[]})
}
async fn platform(State(memory): State<Arc<Mutex<Memory>>>, request: Request<Body>) -> Response {
    let release = (request.uri().path() == "/api/v10/channels/99/messages")
        .then(|| memory.lock().unwrap().history_release_at)
        .flatten();
    let response = platform_response(&memory, request);
    if let Some(release) = release {
        if let Ok(delay) = (release - time::OffsetDateTime::now_utc()).try_into() {
            tokio::time::sleep(delay).await;
        }
    }
    response
}
fn platform_response(memory: &Arc<Mutex<Memory>>, request: Request<Body>) -> Response {
    let uri = request.uri().to_string();
    let mut memory = memory.lock().unwrap();
    memory.calls.push((
        uri.clone(),
        request
            .headers()
            .get("authorization")
            .unwrap()
            .to_str()
            .unwrap()
            .into(),
    ));
    let body = match request.uri().path() {
        "/api/v10/users/@me" => json!({"id":"77","bot":true}),
        "/api/v10/channels/99" => memory.channel.clone(),
        "/api/v10/channels/99/messages/700" => {
            return (memory.anchor_status, Json(history_message("700"))).into_response();
        }
        "/api/v10/channels/99/messages" => {
            assert_eq!(uri, "/api/v10/channels/99/messages?before=700&limit=20");
            if let Some(change) = memory.on_history.take() {
                change();
            }
            return (memory.status, Json(memory.messages.clone())).into_response();
        }
        path if path.starts_with("/provider/") => memory.snapshot.clone(),
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    Json(body).into_response()
}
impl App {
    async fn new() -> Self {
        let backend = Arc::new(fake_cma::FakeCma::start(false).await);
        let records = RecordClient::new(&backend.base_url, "cma-service").unwrap();
        let keys =
            Arc::new(MasterKeyRing::parse(&format!("1:{}", STANDARD.encode([3; 32]))).unwrap());
        let key = format!("nyxid_ag_{}", uuid::Uuid::new_v4().simple());
        let mut definition = fixture::with_agent_key(&key);
        definition.record_id = uuid::Uuid::new_v4().to_string().parse().unwrap();
        definition.sources = vec![Source::Discord {
            bot_id: "77".into(),
            intents: 37377,
            admission: AdmissionPolicy::All {},
            thread_history: true,
            thread_lifecycle: false,
            conversation_history: false,
        }];
        definition.secrets.discord_bot_token =
            Some(cmaeg_core::channel::Secret::new("thread-history-fixture"));
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
        let channel_id = pointer.seal_id(&keys).unwrap();
        let mut activity: Activity = serde_json::from_str(include_str!(
            "../../../cmaeg-core/tests/fixtures/activity.json"
        ))
        .unwrap();
        activity.source = SourceKind::Discord {
            bot_id: "77".into(),
        };
        activity.conversation.id = "99".into();
        activity.conversation.kind = cmaeg_core::activity::ConversationKind::Group;
        activity.conversation.thread_id = Some("99".into());
        activity.event_id = "discord:77:700".parse().unwrap();
        let event = EventRefPayload {
            v: 1,
            telegram_admission: None,
            activity_sha256: Some(activity_digest(&activity)),
            channel_id,
            record_id: definition.record_id.clone(),
            event_id: activity.event_id.clone(),
            partition_key: "discord:99".into(),
            conversation_id: "conv_00000000000000000000000000000001".parse().unwrap(),
            expires_at: time::OffsetDateTime::now_utc().unix_timestamp() + 600,
            reply: ReplyAddress::Discord {
                bot_id: "77".into(),
                channel_id: "99".into(),
                thread_id: Some("99".into()),
                message_id: "700".into(),
            },
            consumer: None,
            lineage: None,
        };
        let memory = Arc::new(Mutex::new(Memory {
            calls: vec![],
            channel: json!({"id":"99","type":11,"guild_id":"100","parent_id":"98"}),
            messages: json!([history_message("699"), history_message("698")]),
            snapshot: Value::Null,
            status: StatusCode::OK,
            anchor_status: StatusCode::OK,
            history_release_at: None,
            on_history: None,
        }));
        let (platform_base, server) = serve(
            Router::new()
                .fallback(get(platform))
                .with_state(memory.clone()),
        )
        .await;
        let provider_slug = definition.consumer(None).unwrap().provider.slug.clone();
        let registry = ProviderRegistry::parse(&json!([{"slug":provider_slug,"base_url":platform_base,"kind":"responses_http","paths":{"agent_card":"/.well-known/agent-card.json","binding":"/provider/{binding_id}","conversation":"/provider/{binding_id}/{conversation_id}","responses":"/api/v2/responses"},"max_inflight":4}]).to_string()).unwrap();
        let (sender, _) = mpsc::channel(1);
        let state = ToolsState {
            complete_replies_enabled: false,
            commonmark_relay: Default::default(),
            telegram_topics: None,
            discord_threads: None,
            native_lark_conversation_history: None,
            discord_conversation_history: None,
            discord_thread_history: Some(Arc::new(DiscordThreadHistory::new(
                crate::discord::DiscordRest::fixture(&format!("{platform_base}/api/v10/")),
            ))),
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
        let mut app = Self {
            backend,
            memory,
            state,
            definition,
            event,
            activity,
            key,
            base: String::new(),
            servers: vec![server],
        };
        app.save();
        app.snapshot();
        app.mount().await;
        app
    }
    fn save(&self) {
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
    fn reference(&self) -> EventRef {
        EventRef::seal(&self.event, &self.state.keys).unwrap()
    }
    fn snapshot(&self) {
        self.memory.lock().unwrap().snapshot = json!({"schema_version":1,"channel_id":self.event.channel_id,"conversation_id":self.event.conversation_id,"activity":self.activity,"event_ref":self.reference()});
    }
    async fn mount(&mut self) {
        let (base, server) = serve(
            crate::http::tools::router(self.state.clone())
                .merge(crate::mcp::router(self.state.clone())),
        )
        .await;
        self.base = base;
        self.servers.push(server);
    }
    async fn read(&self, query: &str) -> reqwest::Response {
        reqwest::Client::new()
            .get(format!(
                "{}/v1/events/{}/context{query}",
                self.base,
                self.reference().as_str()
            ))
            .bearer_auth(&self.key)
            .send()
            .await
            .unwrap()
    }
    fn platform_reads(&self) -> usize {
        self.memory
            .lock()
            .unwrap()
            .calls
            .iter()
            .filter(|(path, _)| path.starts_with("/api/"))
            .count()
    }
}

mod contracts;
