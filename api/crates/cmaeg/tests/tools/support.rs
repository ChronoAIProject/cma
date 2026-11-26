use super::{
    fake_provider::FakeProvider,
    media_support::MediaSetup,
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
    media::service::MediaService,
    providers::{EventContextSnapshot, ProviderClient, ProviderRegistry, TurnRequest},
    tools::{MediaDownloads, ToolsState},
    turns::TurnTracker,
};
use cmaeg_core::{
    activity::{Activity, ActivityRouting},
    ids::MessageRef,
    message::{OutboundMessage, RenderingOutcome, RenderingReport},
    seal::{event_ref::activity_digest, EventRef, MasterKeyRing, MessageRefPayload},
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
    /// The activity the provider stores for `event_ref`.
    pub activity: Activity,
    pub message_ref: MessageRef,
    pub requests: Arc<Mutex<Vec<OutboundMessage>>>,
    pub delay: Arc<AtomicU64>,
    /// The media service, when the app was built with one.
    pub media: Option<Arc<MediaService>>,
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
        Self::build("telegram", text, None, None, false).await
    }
    /// A group message on `platform` carrying the envelope's default text.
    pub async fn on(platform: &str) -> Self {
        Self::build(platform, "hello", None, None, false).await
    }
    pub async fn on_private(platform: &str) -> Self {
        Self::build(platform, "hello", None, None, true).await
    }
    /// A message carrying `setup`'s attachments, with a media service over
    /// its fakes. `event_ref` is the media event's reference.
    pub async fn with_media(setup: MediaSetup) -> Self {
        Self::build("telegram", "hello", Some(setup), None, false).await
    }
    /// A message whose stored activity carries `routing`, bound into its
    /// event_ref's digest as routed ingress binds it.
    pub async fn routed(routing: ActivityRouting) -> Self {
        Self::build("telegram", "hello", None, Some(routing), false).await
    }
    async fn build(
        platform: &str,
        text: &str,
        media: Option<MediaSetup>,
        routing: Option<ActivityRouting>,
        private: bool,
    ) -> Self {
        let key = format!("nyxid_ag_{}", uuid::Uuid::new_v4().simple());
        let edit = media.as_ref().map(|setup| setup.edit);
        let fixture = Fixture::on(
            platform,
            |value| {
                if platform == "whatsapp" {
                    value["sources"][0]["verify_raw_event"] = true.into();
                }
                value["secrets"]["agent_key_hash"] =
                    format!("sha256:{:x}", Sha256::digest(key.as_bytes())).into();
                value["secrets"]["provider_credential"] = key.clone().into();
                if let Some(edit) = edit {
                    edit(value);
                }
            },
            Default::default(),
        )
        .await;
        let mut envelope = Envelope::on(platform);
        if platform == "whatsapp" {
            envelope.payload["conversation"]["platform_id"] = "7".into();
            envelope.payload["raw_platform_data"] =
                json!({"id":"wamid.initial","from":"7","type":"text","text":{"body":"hello"}});
        }
        if !private {
            envelope.group();
        }
        if platform == "telegram" {
            envelope.payload["content"]["text"] = text.into();
            envelope.payload["raw_platform_data"]["message"]["text"] = text.into();
        }
        assert_eq!(fixture.send(&envelope).await.status(), 202);
        let mut request = fixture.recorder.requests.lock().unwrap().pop().unwrap();
        if let Some(setup) = &media {
            request = setup.retarget(request, &fixture.state.master_keys);
        }
        if let Some(routing) = routing {
            request = route(request, routing, &fixture.state.master_keys);
        }
        let event_ref = request.event_ref.clone();
        let activity = request.activity.clone();
        let payload = event_ref
            .open(&fixture.state.master_keys, super::relay_support::now())
            .unwrap()
            .payload;
        let message_ref = MessageRef::seal(
            &MessageRefPayload {
                discord_message: None,
                native_media_edit: None,
                received_reaction: None,
                native_lark_effect: None,
                whatsapp_reaction: None,
                lark_effect: None,
                telegram_effect: None,
                channel_id: fixture.id.clone(),
                event_id: payload.event_id.clone(),
                platform_message_id: "platform-1".into(),
                route_id: "route-1".into(),
                interaction_record_id: None,
                relay_message: None,
                consumer: None,
            },
            &fixture.state.master_keys,
        )
        .unwrap();
        let fake = FakeProvider::start(Default::default()).await;
        let entry = |slug: &str| json!({"slug":slug,"base_url":fake.base_url(),"kind":"responses_http","paths":{"agent_card":"/.well-known/agent-card.json","binding":"/api/v1/cmaeg/bindings/{binding_id}","conversation":"/api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}","responses":"/api/v2/responses"},"max_inflight":1});
        // A second slug lets media tests bind a provider that installs no
        // gateway tools.
        let registry =
            ProviderRegistry::parse(&json!([entry("cma_codex"), entry("nyxbot")]).to_string())
                .unwrap();
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
                            message_parts: vec![],
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
                            message_parts: vec![],
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
        let downloads = Arc::new(
            media
                .as_ref()
                .map_or_else(MediaDownloads::default, MediaSetup::downloads),
        );
        let media = media.map(|setup| Arc::new(setup.service(fixture.state.master_keys.clone())));
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
            resolver: fixture.state.resolver.clone(),
            keys: fixture.state.master_keys.clone(),
            providers,
            delivery: Arc::new(cmaeg::delivery::router::ShardedScheduler::new(delivery)),
            routed_replies: None,
            tracker: TurnTracker::new(http::readiness::Readiness::initialized()),
            public_origin: "https://gateway.example".into(),
            public_api_url: "https://gateway.example".into(),
            public_api_aliases: vec![],
            advertise_public_api_url: true,
            media: media.clone(),
            downloads,
        };
        let router = http::tools::router(state.clone())
            .merge(http::media::router(state.clone()))
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
            activity,
            message_ref,
            requests,
            delay,
            media,
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
    /// GET attachment `index` of the app's event, with `query`.
    pub async fn attachment(&self, index: &str, query: &str) -> reqwest::Response {
        self.attachment_under(self.event_ref.as_str(), index, query)
            .await
    }
    /// GET attachment `index` under `reference`, which may be a resealed,
    /// expired or tampered form of the app's event_ref.
    pub async fn attachment_under(
        &self,
        reference: &str,
        index: &str,
        query: &str,
    ) -> reqwest::Response {
        self.fixture
            .client
            .get(format!(
                "{}/v1/events/{reference}/attachments/{index}{query}",
                self.base
            ))
            .bearer_auth(&self.key)
            .send()
            .await
            .unwrap()
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
/// Bind `routing` into the request's activity and reseal its event_ref over
/// the routed activity's digest.
fn route(
    mut request: LaunchRequest,
    routing: ActivityRouting,
    keys: &MasterKeyRing,
) -> LaunchRequest {
    request.activity.routing = Some(routing);
    let mut payload = request
        .event_ref
        .open(keys, super::relay_support::now())
        .unwrap()
        .payload;
    payload.activity_sha256 = Some(activity_digest(&request.activity));
    request.event_ref = EventRef::seal(&payload, keys).unwrap();
    request
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
                nyxid_trace: false,
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
