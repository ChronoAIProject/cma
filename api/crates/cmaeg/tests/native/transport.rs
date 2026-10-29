use crate::ingress::{message, Native};
use axum::{
    extract::{Path, State},
    routing::post,
    Json, Router,
};
use cmaeg::{
    delivery::{
        engine::DeliveryEngine,
        intents::{DeliveryCommand, DeliveryOutcome, ReplySource, TypingOutcome},
        relay_client::RelayClient,
    },
    http::readiness::Readiness,
    telegram::{Error, TelegramClient},
    turns::TurnTracker,
};
use cmaeg_core::{channel::Secret, message::OutboundMessage};
use serde_json::{json, Value};
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, oneshot};
#[derive(Clone, Default)]
struct Backend {
    calls: Arc<Mutex<Vec<(String, Value)>>>,
    bad: Arc<Mutex<bool>>,
}
struct Fake {
    backend: Backend,
    base: String,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Fake {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Fake {
    async fn start() -> Self {
        let backend = Backend::default();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let app = Router::new()
            .route("/{token}/{method}", post(call))
            .with_state(backend.clone());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            backend,
            base,
            task,
        }
    }
    fn client(&self) -> TelegramClient {
        TelegramClient::new(&self.base, Duration::from_secs(1)).unwrap()
    }
}
async fn call(
    State(state): State<Backend>,
    Path((_token, method)): Path<(String, String)>,
    Json(body): Json<Value>,
) -> Json<Value> {
    state
        .calls
        .lock()
        .unwrap()
        .push((method.clone(), body.clone()));
    if *state.bad.lock().unwrap() {
        return Json(json!({"ok":true,"result":{"message_id":9,"chat":{"id":999}}}));
    }
    let id = |field: &str| {
        body[field]
            .as_i64()
            .or_else(|| body[field].as_str().and_then(|v| v.parse::<i64>().ok()))
            .unwrap()
    };
    let result = match method.as_str() {
        "getMe" => {
            json!({"id":77,"is_bot":true,"first_name":"Manager","username":"manager_bot","can_manage_bots":true})
        }
        "getWebhookInfo" => json!({"url":""}),
        "getManagedBotToken" => {
            assert_eq!(body["user_id"], 88);
            json!("88:child-fixture")
        }
        "setWebhook" | "deleteWebhook" | "sendChatAction" | "answerCallbackQuery" => json!(true),
        "sendMessage" => json!({"message_id":100,"chat":{"id":id("chat_id")}}),
        "editMessageText" => json!({"message_id":id("message_id"),"chat":{"id":id("chat_id")}}),
        _ => panic!("unexpected method"),
    };
    Json(json!({"ok":true,"result":result}))
}
#[tokio::test]
async fn manager_operations_use_native_platform_fields_and_keep_tokens_private() {
    let fake = Fake::start().await;
    let client = fake.client();
    let token = Secret::new("77:fixture");
    let identity = client.get_me(&token).await.unwrap();
    assert_eq!(identity.id, "77");
    assert!(identity.can_manage_bots);
    let child = client.managed_bot_token(&token, "88").await.unwrap();
    assert_eq!(child.as_str(), "88:child-fixture");
    assert_eq!(format!("{child:?}"), "[redacted]");
    client
        .send_managed_keyboard(&token, "42", 17, Some("Paper"), Some("paper_fixture_bot"))
        .await
        .unwrap();
    assert_eq!(client.get_webhook_url(&child).await.unwrap(), "");
    client
        .set_webhook(
            &child,
            "https://gateway.example/v1/telegram/fixture",
            &Secret::new("webhook-fixture"),
        )
        .await
        .unwrap();
    client.delete_webhook(&child).await.unwrap();
    let calls = fake.backend.calls.lock().unwrap();
    let keyboard = &calls
        .iter()
        .find(|(method, _)| method == "sendMessage")
        .unwrap()
        .1;
    assert_eq!(
        keyboard["reply_markup"]["keyboard"][0][0]["request_managed_bot"]["request_id"],
        17
    );
    assert!(keyboard["reply_markup"].get("inline_keyboard").is_none());
    let webhook = &calls
        .iter()
        .find(|(method, _)| method == "setWebhook")
        .unwrap()
        .1;
    assert_eq!(webhook["drop_pending_updates"], false);
    assert_eq!(
        webhook["allowed_updates"],
        json!([
            "message",
            "edited_message",
            "channel_post",
            "edited_channel_post",
            "callback_query",
            "message_reaction",
            "message_reaction_count"
        ])
    );
}
#[tokio::test]
async fn ordinary_native_reply_edit_typing_and_restart_use_sealed_original_chat() {
    let app = Native::new(None).await;
    app.accepted(&message()).await;
    let request = app.fixture.recorder.requests.lock().unwrap().pop().unwrap();
    let event = request
        .event_ref
        .open(&app.fixture.state.master_keys, crate::support::now())
        .unwrap()
        .payload;
    let fake = Fake::start().await;
    for _ in 0..2 {
        let (commands, receiver) = mpsc::channel(16);
        let relay = RelayClient::new(&fake.base, Duration::from_secs(1))
            .unwrap()
            .with_telegram(fake.client());
        let tracker = TurnTracker::new(Readiness::initialized());
        let engine = DeliveryEngine::start(
            receiver,
            app.fixture.state.master_keys.clone(),
            relay,
            Default::default(),
            tracker,
            cmaeg::delivery::custom::CustomDelivery::new(
                "https://gateway.example".into(),
                Arc::default(),
            ),
        );
        let (receipt, received) = oneshot::channel();
        commands
            .send(DeliveryCommand::Reply {
                trace: tracing::Span::current(),
                channel: request.channel.clone(),
                event: event.clone(),
                message: OutboundMessage {
                    whatsapp_template: None,
                    structured_content: None,
                    reply_with_received_sticker: false,
                    clear_native_attachments: false,
                    native_media_update: None,
                    native_attachments: Vec::new(),
                    thread_reply: false,
                    text: Some("ordinary response".into()),
                    blocks: vec![],
                    reply_to: None,
                },
                source: ReplySource::Tool,
                receipt,
            })
            .await
            .unwrap();
        let result = received.await.unwrap();
        assert!(matches!(result.delivery, DeliveryOutcome::Confirmed { .. }));
        let reference = result
            .message_ref
            .unwrap()
            .open(&app.fixture.state.master_keys)
            .unwrap()
            .payload;
        assert_eq!(reference.route_id, "telegram:77:42");
        let (receipt, received) = oneshot::channel();
        commands
            .send(DeliveryCommand::Update {
                trace: tracing::Span::current(),
                channel: request.channel.clone(),
                message_ref: reference,
                message: OutboundMessage {
                    whatsapp_template: None,
                    structured_content: None,
                    reply_with_received_sticker: false,
                    clear_native_attachments: false,
                    native_media_update: None,
                    native_attachments: Vec::new(),
                    thread_reply: false,
                    text: Some("edited response".into()),
                    blocks: vec![],
                    reply_to: None,
                },
                receipt,
            })
            .await
            .unwrap();
        assert!(matches!(
            received.await.unwrap().delivery,
            DeliveryOutcome::Confirmed { .. }
        ));
        let (receipt, received) = oneshot::channel();
        commands
            .send(DeliveryCommand::Typing {
                trace: tracing::Span::current(),
                channel: request.channel.clone(),
                event: event.clone(),
                receipt,
            })
            .await
            .unwrap();
        assert_eq!(received.await.unwrap().outcome, TypingOutcome::Sent);
        drop(commands);
        assert!(engine.drain(Instant::now() + Duration::from_secs(2)).await);
    }
    let calls = fake.backend.calls.lock().unwrap();
    assert_eq!(calls.len(), 6);
    assert!(calls.iter().all(|(_, body)| body["chat_id"] == "42"));
}
#[tokio::test]
async fn contradictory_send_receipt_is_unknown_and_unsafe_token_never_reaches_network() {
    let fake = Fake::start().await;
    let client = fake.client();
    *fake.backend.bad.lock().unwrap() = true;
    assert_eq!(
        client
            .send_message(&Secret::new("77:fixture"), "42", None, "hello", None)
            .await,
        Err(Error::Unconfirmed)
    );
    let error = client
        .get_me(&Secret::new("77:invalid/path"))
        .await
        .unwrap_err();
    assert_eq!(format!("{error:?}"), "Rejected");
    assert_eq!(fake.backend.calls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn native_buttons_deliver_and_consume_with_durable_original_actor_fences() {
    use cmaeg::{
        interactions::InteractionStore,
        providers::{EventContextSnapshot, ProviderClient, ProviderRegistry, TurnRequest},
        records::RecordClient,
    };
    let mut app = Native::new(None).await;
    app.accepted(&message()).await;
    let request = app.fixture.recorder.requests.lock().unwrap().pop().unwrap();
    let provider = crate::fake_provider::FakeProvider::start(Default::default()).await;
    let backend = crate::fake_cma::FakeCma::start(false).await;
    let registry=ProviderRegistry::parse(&json!([{"slug":"cma_codex","base_url":provider.base_url(),"kind":"responses_http","paths":{"agent_card":"/.well-known/agent-card.json","binding":"/api/v1/cmaeg/bindings/{binding_id}","conversation":"/api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}","responses":"/api/v2/responses"},"max_inflight":8}]).to_string()).unwrap();
    let providers = Arc::new(
        ProviderClient::new(registry, Duration::from_secs(1), Duration::from_secs(2), 8).unwrap(),
    );
    let mut stream = providers
        .start_turn(
            &request.channel.definition.provider.slug,
            TurnRequest {
                timing: None,
                conversation_id: request.conversation_id.clone(),
                input: vec![],
                metadata: Default::default(),
                idempotency_key: request.idempotency_key.clone(),
                agent_key: request
                    .channel
                    .definition
                    .secrets
                    .provider_credential
                    .clone()
                    .unwrap(),
                event_context: Some(EventContextSnapshot {
                    schema_version: 1,
                    channel_id: request.channel.id.clone(),
                    conversation_id: request.conversation_id.clone(),
                    activity: request.activity.clone(),
                    event_ref: request.event_ref.clone(),
                }),
            },
        )
        .await
        .unwrap();
    (&mut stream.accepted).await.unwrap();
    while stream.events.recv().await.is_some() {}
    let store = Arc::new(InteractionStore::new(
        RecordClient::new(&backend.base_url, "cma-service").unwrap(),
        app.fixture.state.master_keys.clone(),
        providers,
    ));
    let event = request
        .event_ref
        .open(&app.fixture.state.master_keys, crate::support::now())
        .unwrap()
        .payload;
    let fake = Fake::start().await;
    let (commands, receiver) = mpsc::channel(16);
    let relay = RelayClient::new(&fake.base, Duration::from_secs(1))
        .unwrap()
        .with_telegram(fake.client())
        .with_interactions(store.clone());
    let engine = DeliveryEngine::start(
        receiver,
        app.fixture.state.master_keys.clone(),
        relay,
        Default::default(),
        TurnTracker::new(Readiness::initialized()),
        cmaeg::delivery::custom::CustomDelivery::new(
            "https://gateway.example".into(),
            Arc::default(),
        ),
    );
    let (receipt, received) = oneshot::channel();
    commands.send(DeliveryCommand::Reply{trace:tracing::Span::current(),channel:request.channel,event,message:serde_json::from_value(json!({"text":"Choose","blocks":[{"type":"buttons","id":"choice","options":[{"id":"first","label":"First"}]}]})).unwrap(),source:ReplySource::Tool,receipt}).await.unwrap();
    let result = received.await.unwrap();
    assert!(matches!(result.delivery, DeliveryOutcome::Confirmed { .. }));
    assert!(result.message_ref.is_some());
    let data = fake.backend.calls.lock().unwrap()[0].1["reply_markup"]["inline_keyboard"][0][0]
        ["callback_data"]
        .as_str()
        .unwrap()
        .to_owned();
    app.with_interactions(store).await;
    let mut raw = json!({"update_id":102,"callback_query":{"id":"click-1","data":data,"from":{"id":99,"is_bot":false,"first_name":"Other"},"message":{"message_id":100,"date":crate::support::now(),"chat":{"id":42,"type":"private"},"from":{"id":77,"is_bot":true},"text":"Choose"}}});
    let result = app
        .send(&raw, "native-webhook-fixture")
        .await
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(result["reason"], "interaction_expired_or_invalid");
    raw["callback_query"]["from"]["id"] = 42.into();
    app.accepted(&raw).await;
    {
        let requests = app.fixture.recorder.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert!(
            matches!(&requests[0].activity.kind,cmaeg_core::activity::ActivityKind::Interaction(value)if value.action_id=="first")
        );
    }
    drop(commands);
    assert!(engine.drain(Instant::now() + Duration::from_secs(2)).await);
}

#[tokio::test]
async fn native_update_releases_request_body_before_launch() {
    let app = Native::new(None).await;
    let (body, dropped) = crate::support::probed_body(serde_json::to_vec(&message()).unwrap());
    app.fixture.recorder.expect_released(dropped.clone());
    let request = axum::http::Request::post(format!("/v1/telegram/{}", app.fixture.id))
        .header("content-type", "application/json")
        .header("x-telegram-bot-api-secret-token", "native-webhook-fixture")
        .body(body)
        .unwrap();
    let response = cmaeg::http::telegram::callback(
        State(app.fixture.state.clone()),
        Path(app.fixture.id.to_string()),
        request,
    )
    .await;
    assert_eq!(response.status(), 202);
    assert_eq!(
        app.fixture
            .recorder
            .probed
            .load(std::sync::atomic::Ordering::SeqCst),
        1
    );
    assert!(dropped.load(std::sync::atomic::Ordering::SeqCst));
}
