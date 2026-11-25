//! Native platform fixtures exercise durable provisioning boundaries over HTTP.
#[path = "consent_tests.rs"]
mod consent_tests;
use super::{
    binding,
    model::*,
    store::index_id,
    tests::{connection, owner, Fixture},
    ConnectionService, Error,
};
use crate::{channels::resolver::ResolvedChannel, telegram::TelegramClient};
use axum::{
    extract::{Path, State},
    routing::post,
    Json, Router,
};
use cmaeg_core::{
    activity::{Activity, ConversationKind, SourceKind},
    channel::{ChannelDefinition, Platform, Source},
    ids::{ChannelId, ConversationId},
    seal::ReplyAddress,
};
use serde_json::{json, Value};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

#[derive(Clone)]
struct Peer {
    store: super::store::Store,
    pending_id: String,
    connection_id: String,
    calls: Arc<Mutex<Vec<(String, Value)>>>,
    reserved_before_button: Arc<AtomicBool>,
    manager_enabled: Arc<AtomicBool>,
    child_enabled: Arc<AtomicBool>,
    mismatched_child: Arc<AtomicBool>,
    webhook: Arc<Mutex<String>>,
    lose_webhook_reply: Arc<AtomicBool>,
    lose_delete_reply: Arc<AtomicBool>,
    pause_webhook: Arc<AtomicBool>,
    pause_delete: Arc<AtomicBool>,
    pause_read: Arc<AtomicBool>,
    fail_read: Arc<AtomicBool>,
    rate_limit: Arc<AtomicBool>,
    read_started: Arc<tokio::sync::Notify>,
    read_resume: Arc<tokio::sync::Notify>,
    webhook_started: Arc<tokio::sync::Notify>,
    webhook_resume: Arc<tokio::sync::Notify>,
    webhook_finished: Arc<tokio::sync::Notify>,
}
struct Native {
    fixture: Fixture,
    service: ConnectionService,
    record: Connection,
    channel: Arc<ResolvedChannel>,
    peer: Peer,
    server: tokio::task::JoinHandle<()>,
    api_base: String,
}
impl Drop for Native {
    fn drop(&mut self) {
        self.server.abort();
    }
}
impl Native {
    async fn new() -> Self {
        Self::with_fixture(Fixture::new().await).await
    }
    async fn with_fixture(fixture: Fixture) -> Self {
        let mut raw: Value = serde_json::from_str(include_str!(
            "../../../cmaeg-core/tests/fixtures/channel.json"
        ))
        .unwrap();
        raw["record_id"] = uuid::Uuid::new_v4().to_string().into();
        raw["sources"] = json!([{"type":"telegram","bot_id":"77","bot_username":"manager_fixture_bot","admission":{"type":"all"}}]);
        raw["secrets"]["telegram_bot_token"] = "77:manager-fixture".into();
        raw["secrets"]["telegram_webhook_secret"] = "manager-webhook-fixture".into();
        raw["partition"]["key"] = "conversation_and_sender".into();
        let mut definition: ChannelDefinition = serde_json::from_value(raw).unwrap();
        definition.profile_digest = definition.profile.digest();
        definition.validate().unwrap();
        let id = fixture.retain_channel(&definition).await;
        let mut record = connection("alice", false);
        record.material = None;
        record.origin.actor = "42".into();
        record.origin.conversation.id = "42".into();
        record.origin.conversation.kind = ConversationKind::Private;
        record.origin.source = SourceKind::Telegram {
            platform: Platform::Telegram,
            bot_id: "77".into(),
        };
        record.origin.event.channel_id = id.clone();
        record.origin.event.record_id = definition.record_id.clone();
        record.origin.binding_id = definition.provider.binding_id.to_string();
        record.origin.agent_key_hash = definition.secrets.agent_key_hash.clone();
        record.origin.event.partition_key = "42\u{1f}42".into();
        record.origin.event.conversation_id =
            ConversationId::derive(&fixture.keys, &id, 1, &record.origin.event.partition_key);
        record.origin.event.reply = ReplyAddress::Telegram {
            bot_id: "77".into(),
            chat_id: "42".into(),
            message_id: "90".into(),
            thread_id: None,
        };
        fixture
            .store()
            .save_connection(&record, None)
            .await
            .unwrap();
        let peer = Peer {
            store: fixture.store(),
            pending_id: index_id("manager-actor", &[definition.record_id.as_str(), "42"]),
            connection_id: record.id.clone(),
            calls: Arc::default(),
            reserved_before_button: Arc::default(),
            manager_enabled: Arc::new(AtomicBool::new(true)),
            child_enabled: Arc::default(),
            mismatched_child: Arc::default(),
            webhook: Arc::default(),
            lose_webhook_reply: Arc::default(),
            lose_delete_reply: Arc::default(),
            pause_webhook: Arc::default(),
            pause_delete: Arc::default(),
            pause_read: Arc::default(),
            fail_read: Arc::default(),
            rate_limit: Arc::default(),
            read_started: Arc::default(),
            read_resume: Arc::default(),
            webhook_started: Arc::default(),
            webhook_resume: Arc::default(),
            webhook_finished: Arc::default(),
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let app = Router::new()
            .route("/{token}/{method}", post(platform))
            .route(
                "/provider/bindings/{id}",
                axum::routing::put(binding_provider).delete(binding_provider),
            )
            .with_state(peer.clone());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let mut service = fixture.service();
        service.management.telegram = Some(Arc::new(
            TelegramClient::new(&base, Duration::from_secs(1)).unwrap(),
        ));
        let channel = service
            .management
            .resolver
            .resolve_current(id.as_str())
            .await
            .unwrap();
        Self {
            fixture,
            service,
            record,
            channel,
            peer,
            server,
            api_base: base,
        }
    }
    async fn begin(&self, manager: bool) {
        self.service
            .create_managed(
                &self.record,
                1,
                self.record.origin.event.record_id.as_str(),
                Some("Paper assistant".into()),
                Some("paper_fixture_bot".into()),
                manager,
            )
            .await
            .unwrap();
    }
    fn receipt(&self, actor: i64, date: i64, update_id: i64) -> Activity {
        let raw = json!({"update_id":update_id,"message":{"message_id":101,"date":date,"chat":{"id":actor,"type":"private"},"from":{"id":actor,"is_bot":false,"first_name":"Alice"},"managed_bot_created":{"bot":{"id":88,"is_bot":true,"first_name":"Paper assistant","username":"paper_fixture_bot"}}}});
        let crate::ingress::telegram::Outcome::Admitted(observed) =
            crate::ingress::telegram::observe(raw, "77", Some("manager_fixture_bot"), false, false)
                .unwrap()
        else {
            panic!("fixture must pass native normalization")
        };
        let built = crate::ingress::telegram::launch(
            self.channel.clone(),
            "77",
            *observed,
            &self.fixture.keys,
            Instant::now(),
            now(),
            false,
        )
        .unwrap();
        let crate::ingress::media_guard::Built::Launch(request) = built else {
            panic!("fixture must launch")
        };
        request.activity
    }
    fn methods(&self) -> Vec<String> {
        self.peer
            .calls
            .lock()
            .unwrap()
            .iter()
            .map(|(method, _)| method.clone())
            .collect()
    }
    async fn stored(&self) -> (Connection, u64) {
        self.fixture
            .store()
            .connection(&self.record.id)
            .await
            .unwrap()
    }
    async fn reserve_child(&self) -> (ChannelDefinition, ChannelId) {
        self.begin(false).await;
        self.service
            .observe_creation(&self.channel, &self.receipt(42, now(), 107))
            .await
            .unwrap();
        let (mut record, version) = self.stored().await;
        let mut definition = self.channel.definition.clone();
        definition.record_id = uuid::Uuid::new_v4().to_string().parse().unwrap();
        definition.owner.subject = record.owner.clone();
        definition.profile.metadata.owner.subject = record.owner.clone();
        definition.profile_digest = definition.profile.digest();
        definition.connection_ref = Some(record.id.clone());
        let material = record.material.as_ref().unwrap();
        definition.sources = vec![Source::Telegram {
            topic_lifecycle: false,
            topic_creation: false,
            message_events: false,
            channel_posts: false,
            member_join_chat_ids: Vec::new(),
            bot_id: material.bot_id.clone(),
            bot_username: Some(material.username.clone()),
            admission: cmaeg_core::channel::AdmissionPolicy::All {},
        }];
        definition.secrets.telegram_bot_token =
            Some(cmaeg_core::channel::Secret::new(material.token.clone()));
        definition.secrets.telegram_webhook_secret = Some(cmaeg_core::channel::Secret::new(
            material.webhook_secret.clone(),
        ));
        record.channel_record = Some(definition.record_id.to_string());
        self.fixture
            .store()
            .save_connection(&record, Some(version))
            .await
            .unwrap();
        let channel_id = self.fixture.retain_channel(&definition).await;
        (definition, channel_id)
    }
    async fn expire_lease(&self) {
        let (mut record, version) = self.stored().await;
        record.webhook_lease.as_mut().unwrap().expires_at = now() - 1;
        self.fixture
            .store()
            .save_connection(&record, Some(version))
            .await
            .unwrap();
    }
    fn replacement_request(
        &self,
        definition: &ChannelDefinition,
    ) -> crate::channels::ChannelCreateRequest {
        serde_json::from_value(json!({
            "schema_version":1,"record_id":uuid::Uuid::new_v4().to_string(),"name":"Replacement",
            "profile":definition.profile,"provider":{"slug":definition.provider.slug},"sources":[],
            "connection_ref":self.record.id,"connection_admission":{"type":"all"},
            "partition":definition.partition,"reply":definition.reply,"credentials":{"agent_key":"fixture-agent-key"}
        })).unwrap()
    }
}
async fn platform(
    State(peer): State<Peer>,
    Path((token, method)): Path<(String, String)>,
    Json(body): Json<Value>,
) -> Json<Value> {
    let manager = token == "bot77:manager-fixture";
    peer.calls
        .lock()
        .unwrap()
        .push((method.clone(), body.clone()));
    if matches!(method.as_str(), "setWebhook" | "deleteWebhook")
        && peer.rate_limit.swap(false, Ordering::SeqCst)
    {
        return Json(json!({"ok":false,"error_code":429,"parameters":{"retry_after":7}}));
    }
    let result = match method.as_str() {
        "getMe" => {
            let id = if manager {
                77
            } else if peer.mismatched_child.load(Ordering::SeqCst) {
                99
            } else {
                88
            };
            json!({"id":id,"is_bot":true,"first_name":if manager{"Manager"}else{"Paper assistant"},"username":if manager{"manager_fixture_bot"}else{"paper_fixture_bot"},"can_manage_bots":if manager{peer.manager_enabled.load(Ordering::SeqCst)}else{peer.child_enabled.load(Ordering::SeqCst)}})
        }
        "getManagedBotToken" => {
            assert!(manager);
            assert_eq!(body["user_id"], 88);
            json!("88:child-fixture")
        }
        "sendMessage" => {
            let (connection, _) = peer.store.connection(&peer.connection_id).await.unwrap();
            let (pending, _) = peer.store.load(&peer.pending_id).await.unwrap();
            let durable = connection.status == Status::AwaitingPlatform
                && matches!(pending,Stored::Pending(pending)if pending.connection_id==connection.id && pending.actor=="42");
            peer.reserved_before_button.store(durable, Ordering::SeqCst);
            assert!(body["reply_markup"]["keyboard"][0][0]
                .get("request_managed_bot")
                .is_some());
            json!({"message_id":100,"chat":{"id":42}})
        }
        "getWebhookInfo" => {
            if peer.pause_read.swap(false, Ordering::SeqCst) {
                peer.read_started.notify_one();
                peer.read_resume.notified().await;
            }
            if peer.fail_read.swap(false, Ordering::SeqCst) {
                return Json(json!({"ok": false}));
            }
            json!({"url":peer.webhook.lock().unwrap().clone()})
        }
        "setWebhook" => {
            if peer.pause_webhook.swap(false, Ordering::SeqCst) {
                peer.webhook_started.notify_one();
                // A remote operation can outlive its caller's TCP connection.
                // Keep this side effect alive when the gateway task is aborted.
                let remote = peer.clone();
                let url = body["url"].as_str().unwrap().to_owned();
                let (finished, wait) = tokio::sync::oneshot::channel();
                tokio::spawn(async move {
                    remote.webhook_resume.notified().await;
                    *remote.webhook.lock().unwrap() = url;
                    remote.webhook_finished.notify_one();
                    let _ = finished.send(());
                });
                wait.await.unwrap();
            } else {
                *peer.webhook.lock().unwrap() = body["url"].as_str().unwrap().into();
                peer.webhook_finished.notify_one();
            }
            assert_eq!(body["drop_pending_updates"], false);
            if peer.lose_webhook_reply.swap(false, Ordering::SeqCst) {
                return Json(json!({"ok":false}));
            }
            json!(true)
        }
        "deleteWebhook" => {
            if peer.pause_delete.swap(false, Ordering::SeqCst) {
                peer.webhook_started.notify_one();
                let remote = peer.clone();
                let (finished, wait) = tokio::sync::oneshot::channel();
                tokio::spawn(async move {
                    remote.webhook_resume.notified().await;
                    remote.webhook.lock().unwrap().clear();
                    remote.webhook_finished.notify_one();
                    let _ = finished.send(());
                });
                wait.await.unwrap();
            } else {
                peer.webhook.lock().unwrap().clear();
            }
            if peer.lose_delete_reply.swap(false, Ordering::SeqCst) {
                return Json(json!({"ok":false}));
            }
            json!(true)
        }
        _ => panic!("unexpected platform operation"),
    };
    Json(json!({"ok":true,"result":result}))
}

#[path = "platform_cases.rs"]
mod cases;

#[path = "webhook_custody_tests.rs"]
mod custody_tests;

#[path = "webhook_native_tests.rs"]
mod native_tests;

async fn binding_provider(
    State(peer): State<Peer>,
    Path(id): Path<String>,
    method: axum::http::Method,
) -> Json<Value> {
    peer.calls
        .lock()
        .unwrap()
        .push((format!("provider_{method}"), json!({})));
    Json(json!({"binding_id":id,"level":"l1","honoured":[],"refused":[]}))
}

#[path = "channel_custody_native_tests.rs"]
mod channel_custody_native_tests;
