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
            discord_thread_history: Some(Arc::new(DiscordThreadHistory::new(
                crate::discord::DiscordRest::fixture(&format!("{platform_base}/api/v10/")),
            ))),
            discord_interactions_enabled: false,
            telegram_effects_issuance_enabled: false,
            native_lark_effects: None,
            lark_effects_issuance_enabled: false,
            lark_effects: None,
            telegram_effects: None,
            whatsapp_templates_enabled: false,
            whatsapp_interactions_enabled: false,
            native_whatsapp_interactions_enabled: false,
            consumer_interactions_enabled: false,
            native_media_enabled: false,
            native_lark_outbound_media_enabled: false,
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

#[tokio::test]
async fn thread_context_reads_only_on_request_and_retains_chronological_edited_bot_text() {
    let app = App::new().await;
    let ordinary: Value = app.read("").await.json().await.unwrap();
    assert_eq!(ordinary["capabilities"]["thread_history"], true);
    assert!(ordinary.get("thread_history").is_none());
    assert_eq!(app.platform_reads(), 0);
    app.memory.lock().unwrap().messages[0]["author"]["bot"] = true.into();
    app.memory.lock().unwrap().messages[0]["edited_timestamp"] = "2026-10-27T01:01:00Z".into();
    let response = app.read("?include_thread_history=true").await;
    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["thread_history"]["messages"][0]["message_id"], "698");
    assert_eq!(
        body["thread_history"]["messages"][1]["author"]["kind"],
        "bot"
    );
    assert_eq!(
        body["thread_history"]["messages"][1]["edited_at"],
        "2026-10-27T01:01:00Z"
    );
    assert_eq!(app.platform_reads(), 4);
    for (path, auth) in &app.memory.lock().unwrap().calls {
        if path.starts_with("/api/") {
            assert_eq!(auth, "Bot thread-history-fixture");
        }
    }
}

#[tokio::test]
async fn thread_context_refuses_unknown_query_caller_destinations_and_missing_gate_or_source_consent(
) {
    let mut app = App::new().await;
    for query in [
        "?include_thread_history=true&channel_id=88",
        "?include_thread_history=true&before=1",
        "?include_thread_history=invalid",
    ] {
        assert_eq!(app.read(query).await.status(), 400);
    }
    assert_eq!(app.platform_reads(), 0);
    app.state.discord_thread_history = None;
    app.mount().await;
    assert_eq!(app.read("?include_thread_history=true").await.status(), 403);
    let body: Value = app.read("").await.json().await.unwrap();
    assert!(body["capabilities"].get("thread_history").is_none());
    let mut app = App::new().await;
    let Source::Discord { thread_history, .. } = &mut app.definition.sources[0] else {
        panic!()
    };
    *thread_history = false;
    app.save();
    assert_eq!(app.read("?include_thread_history=true").await.status(), 403);
    assert_eq!(app.platform_reads(), 0);
}

#[tokio::test]
async fn thread_context_refuses_foreign_thread_missing_proof_and_stale_consumer_key() {
    for case in 0..4 {
        let mut app = App::new().await;
        match case {
            0 => {
                app.event.activity_sha256 = None;
            }
            1 => {
                if let ReplyAddress::Discord { thread_id, .. } = &mut app.event.reply {
                    *thread_id = None;
                }
            }
            2 => {
                fixture::replace_default_agent_key(
                    &mut app.definition,
                    "replacement-agent-fixture",
                );
                app.save();
            }
            _ => {
                app.memory.lock().unwrap().channel["id"] = "88".into();
            }
        }
        app.snapshot();
        assert!(
            !app.read("?include_thread_history=true")
                .await
                .status()
                .is_success(),
            "case {case}"
        );
        assert!(!app
            .memory
            .lock()
            .unwrap()
            .calls
            .iter()
            .any(|(path, _)| path.contains("?before=")));
    }
}

#[tokio::test]
async fn thread_context_checks_authority_again_after_read_and_never_returns_revoked_data() {
    for case in 0..3 {
        let app = App::new().await;
        let backend = app.backend.clone();
        let keys = app.state.keys.clone();
        let mut changed = app.definition.clone();
        match case {
            0 => {
                changed.secrets.discord_bot_token =
                    Some(cmaeg_core::channel::Secret::new("rotated-thread-fixture"))
            }
            1 => {
                let Source::Discord { thread_history, .. } = &mut changed.sources[0] else {
                    panic!()
                };
                *thread_history = false;
            }
            _ => fixture::replace_default_agent_key(&mut changed, "revoked-agent-fixture"),
        }
        app.memory.lock().unwrap().on_history = Some(Box::new(move || {
            backend.seed(
                changed.record_id.as_str(),
                &changed.owner.subject,
                seal_blob(&keys, &serde_json::to_vec(&changed.to_blob_json()).unwrap()).unwrap(),
            )
        }));
        assert_eq!(
            app.read("?include_thread_history=true").await.status(),
            403,
            "case {case}"
        );
    }
}

#[tokio::test]
async fn thread_context_is_same_explicit_operation_over_mcp() {
    let app = App::new().await;
    let response=reqwest::Client::new().post(format!("{}/mcp",app.base)).bearer_auth(&app.key).json(&json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"readEventContext","arguments":{"event_ref":app.reference(),"include_thread_history":true}}})).send().await.unwrap();
    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.unwrap();
    assert!(body.to_string().contains("before_message_id"), "{body}");
    assert_eq!(app.platform_reads(), 4);
}

#[tokio::test]
async fn thread_context_refuses_oversized_cross_channel_future_duplicate_and_redirected_results() {
    for case in 0..6 {
        let app = App::new().await;
        {
            let mut memory = app.memory.lock().unwrap();
            match case {
                0 => {
                    memory.messages = json!((0..21)
                        .map(|id| history_message(&(600 + id).to_string()))
                        .collect::<Vec<_>>())
                }
                1 => memory.messages[0]["channel_id"] = "88".into(),
                2 => memory.messages[0]["id"] = "700".into(),
                3 => memory.messages[1]["id"] = "699".into(),
                4 => memory.status = StatusCode::TEMPORARY_REDIRECT,
                _ => memory.messages = Value::String("x".repeat(1024 * 1024)),
            }
        }
        assert!(
            !app.read("?include_thread_history=true")
                .await
                .status()
                .is_success(),
            "case {case}"
        );
        assert_eq!(app.platform_reads(), 4);
    }
}

#[tokio::test]
async fn thread_context_bounds_unicode_text_and_never_exports_attachment_authority() {
    let app = App::new().await;
    app.memory.lock().unwrap().messages = json!((690..696).map(|id| {
        let mut message = history_message(&id.to_string());
        message["content"] = format!("\u{1b}{}", "界".repeat(9000)).into();
        message["attachments"] = json!([{"id":"123", "url":"https://cdn.example/private-file", "proxy_url":"https://proxy.example/private-file"}]);
        message
    }).collect::<Vec<_>>());
    let response = app.read("?include_thread_history=true").await;
    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.unwrap();
    let history = &body["thread_history"];
    assert_eq!(history["text_truncated"], true);
    let messages = history["messages"].as_array().unwrap();
    let total: usize = messages
        .iter()
        .map(|message| {
            let text = message["text"].as_str().unwrap();
            assert!(text.len() <= 8192);
            assert!(!text.contains('\u{1b}'));
            assert_eq!(message["attachment_count"], 1);
            text.len()
        })
        .sum();
    assert!(total <= 32768 && total > 24000);
    assert!(!history.to_string().contains("private-file"));
    assert!(!history.to_string().contains("message_ref"));
}

#[tokio::test]
async fn thread_context_accepts_verified_thread_types_and_refuses_parent_or_missing_anchor() {
    for kind in [10, 11, 12, 0] {
        let app = App::new().await;
        app.memory.lock().unwrap().channel["type"] = kind.into();
        let status = app.read("?include_thread_history=true").await.status();
        assert_eq!(status, if kind == 0 { 403 } else { 200 }, "kind {kind}");
        assert_eq!(app.platform_reads(), if kind == 0 { 2 } else { 4 });
    }
    let app = App::new().await;
    app.memory.lock().unwrap().anchor_status = StatusCode::NOT_FOUND;
    assert_eq!(app.read("?include_thread_history=true").await.status(), 403);
    assert_eq!(app.platform_reads(), 3);
}

#[tokio::test]
async fn thread_context_rate_limit_is_not_retried_and_cools_down_following_reads() {
    let app = App::new().await;
    app.memory.lock().unwrap().status = StatusCode::TOO_MANY_REQUESTS;
    app.memory.lock().unwrap().messages = json!({"retry_after": 60});
    assert_eq!(app.read("?include_thread_history=true").await.status(), 429);
    assert_eq!(app.platform_reads(), 4);
    assert_eq!(app.read("?include_thread_history=true").await.status(), 429);
    assert_eq!(app.platform_reads(), 4);
}

#[tokio::test]
async fn thread_context_preserves_named_consumer_binding_and_rejects_default_key() {
    let mut app = App::new().await;
    let reviewer = "csm_00000000000a";
    let reviewer_key = format!("nyxid_ag_{}", uuid::Uuid::new_v4().simple());
    let hash = crate::channels::definition::hash_secret(&reviewer_key);
    let default = app.definition.consumer(None).unwrap();
    app.definition.routing = Some(serde_json::from_value(json!({
        "version":1,"default_consumer":{"name":"lead"},"default_targets":["default"],
        "consumers":[{"id":reviewer,"name":"reviewer","profile":default.profile,
            "profile_digest":default.profile.digest(),
            "provider":{"slug":default.provider.slug,"binding_id":"binding-reviewer","level":"l1"},
            "agent_key_hash":hash,"created_at":"2026-10-01T00:00:00Z"}]
    })).unwrap());
    app.definition.secrets.consumer_keys = vec![(
        cmaeg_core::ids::ConsumerId::parse(reviewer).unwrap(),
        cmaeg_core::channel::Secret::new(&reviewer_key),
    )];
    app.definition.validate().unwrap();
    app.event.consumer = Some(cmaeg_core::ids::ConsumerId::parse(reviewer).unwrap());
    app.save();
    app.snapshot();
    assert_eq!(app.read("?include_thread_history=true").await.status(), 403);
    assert_eq!(app.platform_reads(), 0);
    app.key = reviewer_key.clone();
    let response = app.read("?include_thread_history=true").await;
    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["consumer"]["id"], reviewer);
    assert_eq!(body["provider_binding_id"], "binding-reviewer");
    let memory = app.memory.lock().unwrap();
    let reads: Vec<_> = memory
        .calls
        .iter()
        .filter(|(path, _)| path.starts_with("/provider/"))
        .collect();
    assert_eq!(reads.len(), 1);
    assert!(reads[0].0.starts_with("/provider/binding-reviewer/"));
    assert_eq!(reads[0].1, format!("Bearer {reviewer_key}"));
}

#[tokio::test]
async fn thread_context_refuses_expiry_during_the_final_authority_read() {
    let mut app = App::new().await;
    app.event.expires_at = time::OffsetDateTime::now_utc().unix_timestamp() + 2;
    app.snapshot();
    app.memory.lock().unwrap().history_release_at = Some(
        time::OffsetDateTime::from_unix_timestamp(app.event.expires_at).unwrap()
            - time::Duration::milliseconds(400),
    );
    let delay = app.backend.get_delay_ms.clone();
    app.memory.lock().unwrap().on_history = Some(Box::new(move || {
        delay.store(600, std::sync::atomic::Ordering::SeqCst);
    }));
    let response = app.read("?include_thread_history=true").await;
    assert_eq!(response.status(), 410);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["code"], "event_ref_expired");
    assert_eq!(app.platform_reads(), 4);
}
