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
}
impl Drop for Native {
    fn drop(&mut self) {
        self.server.abort();
    }
}
impl Native {
    async fn new() -> Self {
        let fixture = Fixture::new().await;
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
        let id = fixture.retain_channel(&definition);
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
            webhook_started: Arc::default(),
            webhook_resume: Arc::default(),
            webhook_finished: Arc::default(),
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let app = Router::new()
            .route("/{token}/{method}", post(platform))
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
            message_events: false,
            channel_posts: false,
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
        let channel_id = self.fixture.retain_channel(&definition);
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
        "getWebhookInfo" => json!({"url":peer.webhook.lock().unwrap().clone()}),
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
            peer.webhook.lock().unwrap().clear();
            if peer.lose_delete_reply.swap(false, Ordering::SeqCst) {
                return Json(json!({"ok":false}));
            }
            json!(true)
        }
        _ => panic!("unexpected platform operation"),
    };
    Json(json!({"ok":true,"result":result}))
}

#[tokio::test]
async fn managed_creation_reserves_before_button_then_verifies_exact_child_and_keeps_token_private()
{
    let app = Native::new().await;
    app.begin(false).await;
    assert!(app.peer.reserved_before_button.load(Ordering::SeqCst));
    let receipt = app.receipt(42, now(), 101);
    let cmaeg_core::activity::ActivityKind::Event { ce, .. } = &receipt.kind else {
        unreachable!()
    };
    assert_eq!(ce.data.as_ref().unwrap()["bot"]["id"], "88");
    app.service
        .observe_creation(&app.channel, &receipt)
        .await
        .unwrap();
    let (record, _) = app.stored().await;
    assert_eq!(record.status, Status::Ready);
    assert_eq!(record.material.as_ref().unwrap().bot_id, "88");
    assert_eq!(
        app.methods(),
        [
            "getMe",
            "sendMessage",
            "getMe",
            "getManagedBotToken",
            "getMe"
        ]
    );
    let view = serde_json::to_string(
        &app.service
            .get(&owner("alice", false), &record.id)
            .await
            .unwrap(),
    )
    .unwrap();
    for secret in [
        "88:child-fixture",
        "77:manager-fixture",
        &record.material.as_ref().unwrap().webhook_secret,
    ] {
        assert!(!view.contains(secret));
    }
    let before = app.methods();
    app.service
        .observe_creation(&app.channel, &receipt)
        .await
        .unwrap();
    assert_eq!(app.methods(), before);
}

#[tokio::test]
async fn other_actor_old_and_cancelled_receipts_cannot_complete_pending_creation() {
    let app = Native::new().await;
    app.begin(false).await;
    let before = app.methods();
    app.service
        .observe_creation(&app.channel, &app.receipt(43, now(), 102))
        .await
        .unwrap();
    app.service
        .observe_creation(
            &app.channel,
            &app.receipt(42, app.record.created_at - 1, 103),
        )
        .await
        .unwrap();
    assert_eq!(app.methods(), before);
    assert_eq!(app.stored().await.0.status, Status::AwaitingPlatform);
    app.service
        .cancel(&owner("alice", false), &app.record.id)
        .await
        .unwrap();
    app.service
        .observe_creation(&app.channel, &app.receipt(42, now(), 104))
        .await
        .unwrap();
    assert_eq!(app.methods(), before);
    assert_eq!(app.stored().await.0.status, Status::Cancelled);
}

#[tokio::test]
async fn child_identity_mismatch_retains_receipt_and_retry_uses_the_same_operation() {
    let app = Native::new().await;
    app.begin(false).await;
    let receipt = app.receipt(42, now(), 105);
    app.peer.mismatched_child.store(true, Ordering::SeqCst);
    assert!(matches!(
        app.service.observe_creation(&app.channel, &receipt).await,
        Err(Error::Invalid(_))
    ));
    let (record, _) = app.stored().await;
    assert_eq!(record.status, Status::AwaitingPlatform);
    assert!(record.material.is_none());
    let (Stored::Pending(pending), _) = app
        .fixture
        .store()
        .load(&app.peer.pending_id)
        .await
        .unwrap()
    else {
        unreachable!()
    };
    assert_eq!(pending.receipt.unwrap().event_id, receipt.event_id.as_str());
    app.peer.mismatched_child.store(false, Ordering::SeqCst);
    app.service
        .observe_creation(&app.channel, &receipt)
        .await
        .unwrap();
    assert_eq!(app.stored().await.0.status, Status::Ready);
}

#[tokio::test]
async fn child_requested_as_manager_waits_for_real_management_capability() {
    let app = Native::new().await;
    app.begin(true).await;
    app.service
        .observe_creation(&app.channel, &app.receipt(42, now(), 106))
        .await
        .unwrap();
    let (record, version) = app.stored().await;
    assert_eq!(record.status, Status::AwaitingPlatform);
    assert!(record.material.is_some());
    let (record, version) = app
        .service
        .refresh_management(record, version)
        .await
        .unwrap();
    assert_eq!(record.status, Status::AwaitingPlatform);
    app.peer.child_enabled.store(true, Ordering::SeqCst);
    let (record, _) = app
        .service
        .refresh_management(record, version)
        .await
        .unwrap();
    assert_eq!(record.status, Status::Ready);
    assert!(record.material.unwrap().can_manage_bots);
}

#[tokio::test]
async fn disabled_parent_management_refuses_before_pending_or_platform_button() {
    let app = Native::new().await;
    app.peer.manager_enabled.store(false, Ordering::SeqCst);
    assert!(matches!(
        app.service
            .create_managed(
                &app.record,
                1,
                app.record.origin.event.record_id.as_str(),
                None,
                None,
                false
            )
            .await,
        Err(Error::Invalid(_))
    ));
    assert_eq!(app.methods(), ["getMe"]);
    assert!(matches!(
        app.fixture.store().load(&app.peer.pending_id).await,
        Err(Error::NotFound)
    ));
    assert_eq!(app.stored().await.0.status, Status::Draft);
}

#[tokio::test]
async fn existing_webhook_is_preserved_and_uncertain_activation_retries_the_retained_pointer() {
    let app = Native::new().await;
    let (definition, channel_id) = app.reserve_child().await;
    *app.peer.webhook.lock().unwrap() = "https://external.example/keep-this-route".into();
    assert!(
        binding::activate(&app.service.management, &definition, &channel_id)
            .await
            .is_err()
    );
    assert_eq!(
        *app.peer.webhook.lock().unwrap(),
        "https://external.example/keep-this-route"
    );
    assert!(!app.methods().contains(&"setWebhook".into()));
    assert_eq!(
        app.stored().await.0.channel_id.as_deref(),
        Some(channel_id.as_str())
    );
    app.peer.webhook.lock().unwrap().clear();
    app.peer.lose_webhook_reply.store(true, Ordering::SeqCst);
    assert!(
        binding::activate(&app.service.management, &definition, &channel_id)
            .await
            .is_err()
    );
    let expected = format!("https://gateway.example/v1/telegram/{channel_id}");
    assert_eq!(*app.peer.webhook.lock().unwrap(), expected);
    assert!(
        binding::activate(&app.service.management, &definition, &channel_id)
            .await
            .is_err()
    );
    app.expire_lease().await;
    binding::activate(&app.service.management, &definition, &channel_id)
        .await
        .unwrap();
    assert_eq!(
        app.stored().await.0.channel_id.as_deref(),
        Some(channel_id.as_str())
    );
    let other: ChannelId = app.fixture.retain_channel(&definition);
    assert_ne!(other, channel_id);
    assert!(
        binding::activate(&app.service.management, &definition, &other)
            .await
            .is_err()
    );
    let urls: Vec<_> = app
        .peer
        .calls
        .lock()
        .unwrap()
        .iter()
        .filter(|(method, _)| method == "setWebhook")
        .map(|(_, body)| body["url"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(urls, [expected.clone(), expected]);
}

#[tokio::test]
async fn deletion_fences_delayed_activation_and_keeps_retirement_until_it_finishes() {
    let app = Native::new().await;
    let (definition, channel_id) = app.reserve_child().await;
    app.peer.pause_webhook.store(true, Ordering::SeqCst);
    let state = app.service.management.clone();
    let task_definition = definition.clone();
    let task_id = channel_id.clone();
    let activating =
        tokio::spawn(async move { binding::activate(&state, &task_definition, &task_id).await });
    tokio::time::timeout(Duration::from_secs(2), app.peer.webhook_started.notified())
        .await
        .unwrap();
    binding::disconnect(&app.service.management, &definition, &channel_id)
        .await
        .unwrap();
    app.fixture.forget_channel(definition.record_id.as_str());
    binding::release(&app.service.management, &definition)
        .await
        .unwrap();
    let (retiring, _) = app.stored().await;
    assert!(retiring.webhook_retiring);
    assert!(retiring.webhook_lease.is_some());
    assert_eq!(retiring.channel_id.as_deref(), Some(channel_id.as_str()));
    assert!(binding::expand_create(
        &app.service.management,
        &owner("alice", false),
        &mut app.replacement_request(&definition)
    )
    .await
    .is_err());
    app.peer.webhook_resume.notify_one();
    assert!(activating.await.unwrap().is_err());
    assert!(app.peer.webhook.lock().unwrap().is_empty());
    let (retired, _) = app.stored().await;
    assert!(!retired.webhook_retiring);
    assert!(retired.webhook_lease.is_none());
    assert!(retired.channel_id.is_none());
    assert!(retired.channel_record.is_none());
    binding::expand_create(
        &app.service.management,
        &owner("alice", false),
        &mut app.replacement_request(&definition),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn restart_reconciles_abandoned_activation_and_uncertain_delete_before_rebinding() {
    let app = Native::new().await;
    let (definition, channel_id) = app.reserve_child().await;
    app.peer.pause_webhook.store(true, Ordering::SeqCst);
    let state = app.service.management.clone();
    let task_definition = definition.clone();
    let task_id = channel_id.clone();
    let activating =
        tokio::spawn(async move { binding::activate(&state, &task_definition, &task_id).await });
    tokio::time::timeout(Duration::from_secs(2), app.peer.webhook_started.notified())
        .await
        .unwrap();
    activating.abort();
    assert!(activating.await.unwrap_err().is_cancelled());
    binding::disconnect(&app.service.management, &definition, &channel_id)
        .await
        .unwrap();
    app.fixture.forget_channel(definition.record_id.as_str());
    binding::release(&app.service.management, &definition)
        .await
        .unwrap();
    app.peer.webhook_resume.notify_one();
    tokio::time::timeout(Duration::from_secs(2), app.peer.webhook_finished.notified())
        .await
        .unwrap();
    assert_eq!(
        *app.peer.webhook.lock().unwrap(),
        format!("https://gateway.example/v1/telegram/{channel_id}")
    );
    let mut restarted = app.fixture.service();
    restarted.management.telegram = app.service.management.telegram.clone();
    assert!(matches!(
        restarted.reconcile_webhook_retirement(&app.record.id).await,
        Err(Error::Conflict)
    ));
    app.expire_lease().await;
    app.peer.lose_delete_reply.store(true, Ordering::SeqCst);
    assert!(matches!(
        restarted.reconcile_webhook_retirement(&app.record.id).await,
        Err(Error::Unavailable)
    ));
    let (uncertain, _) = app.stored().await;
    assert!(uncertain.webhook_retiring);
    assert_eq!(uncertain.channel_id.as_deref(), Some(channel_id.as_str()));
    assert!(matches!(
        restarted.reconcile_webhook_retirement(&app.record.id).await,
        Err(Error::Conflict)
    ));
    app.expire_lease().await;
    restarted
        .reconcile_webhook_retirement(&app.record.id)
        .await
        .unwrap();
    let (retired, _) = app.stored().await;
    assert!(retired.channel_id.is_none());
    assert!(retired.webhook_lease.is_none());
    assert!(!retired.webhook_retiring);
    assert!(app.peer.webhook.lock().unwrap().is_empty());
    binding::expand_create(
        &restarted.management,
        &owner("alice", false),
        &mut app.replacement_request(&definition),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn canonical_migration_preserves_and_can_retire_the_exact_legacy_webhook() {
    let mut app = Native::new().await;
    app.service.management.public_api_url = "https://bot.example/event-gateway".into();
    let (definition, channel_id) = app.reserve_child().await;
    let legacy = format!("https://gateway.example/v1/telegram/{channel_id}");
    *app.peer.webhook.lock().unwrap() = legacy.clone();
    binding::activate(&app.service.management, &definition, &channel_id)
        .await
        .unwrap();
    assert_eq!(*app.peer.webhook.lock().unwrap(), legacy);
    assert!(!app.methods().contains(&"setWebhook".into()));
    binding::disconnect(&app.service.management, &definition, &channel_id)
        .await
        .unwrap();
    assert!(app.peer.webhook.lock().unwrap().is_empty());
    assert!(app.methods().contains(&"deleteWebhook".into()));
}

#[tokio::test]
async fn canonical_migration_installs_new_webhook_and_rejects_other_channel_urls() {
    let mut app = Native::new().await;
    app.service.management.public_api_url = "https://bot.example/event-gateway".into();
    let (definition, channel_id) = app.reserve_child().await;
    for foreign in [
        "https://gateway.example/v1/telegram/another-channel",
        "https://bot.example/event-gateway/v1/telegram/another-channel",
    ] {
        *app.peer.webhook.lock().unwrap() = foreign.into();
        assert!(
            binding::activate(&app.service.management, &definition, &channel_id)
                .await
                .is_err()
        );
        assert_eq!(*app.peer.webhook.lock().unwrap(), foreign);
    }
    app.peer.webhook.lock().unwrap().clear();
    binding::activate(&app.service.management, &definition, &channel_id)
        .await
        .unwrap();
    assert_eq!(
        *app.peer.webhook.lock().unwrap(),
        format!("https://bot.example/event-gateway/v1/telegram/{channel_id}")
    );
}

#[tokio::test]
async fn phase_one_advertisement_installs_legacy_but_preserves_existing_canonical_webhook() {
    let mut app = Native::new().await;
    app.service.management.public_api_url = "https://bot.example/event-gateway".into();
    app.service.management.advertise_public_api_url = false;
    let (definition, channel_id) = app.reserve_child().await;
    let canonical = format!("https://bot.example/event-gateway/v1/telegram/{channel_id}");
    *app.peer.webhook.lock().unwrap() = canonical.clone();
    binding::activate(&app.service.management, &definition, &channel_id)
        .await
        .unwrap();
    assert_eq!(*app.peer.webhook.lock().unwrap(), canonical);
    assert!(!app.methods().contains(&"setWebhook".into()));
    app.peer.webhook.lock().unwrap().clear();
    binding::activate(&app.service.management, &definition, &channel_id)
        .await
        .unwrap();
    assert_eq!(
        *app.peer.webhook.lock().unwrap(),
        format!("https://gateway.example/v1/telegram/{channel_id}")
    );
}
