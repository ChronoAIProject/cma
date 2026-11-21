use super::*;
use crate::{channels::routing_fixture, delivery::destination::DefaultDestination};
use cmaeg_core::channel::DefaultReply;

fn configured(channel: &mut Arc<ResolvedChannel>, reply: serde_json::Value) -> DefaultDestination {
    let mut definition = routing_fixture::routed();
    definition["routing"]["default_reply"] = reply;
    Arc::get_mut(channel).unwrap().definition = routing_fixture::definition(definition);
    DefaultDestination::capture(channel).unwrap()
}

#[tokio::test]
async fn default_relay_uses_initiated_route_and_exact_idempotency_not_origin_message() {
    let (sender, transport, mut channel, event) = setup();
    let destination = configured(
        &mut channel,
        json!({"type":"relay_conversation", "source_id":"relay_lark", "route_id":routing_fixture::ROUTE}),
    );
    let gate = RecordingGate::new(true);
    let trace = tracing::Span::none();
    let receipt = sender
        .default_reply(
            &SendContext {
                trace: &trace,
                channel: &channel,
                first_send_done: true,
                gate: &gate,
            },
            &event,
            &message("answer".into()),
            &destination,
            "stable-operation",
        )
        .await;
    assert!(matches!(
        receipt.delivery,
        DeliveryOutcome::Confirmed { .. }
    ));
    let sent = transport.1.lock().unwrap();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].0.path(), "/api/v1/channel-relay/send");
    assert_eq!(sent[0].1["conversation_id"], routing_fixture::ROUTE);
    assert_eq!(sent[0].1["idempotency_key"], "stable-operation");
    assert!(sent[0].1.get("message_id").is_none());
    assert!(!sent[0].1.to_string().contains("single-use-test-fixture"));
    assert_eq!(
        gate.calls(),
        ["enter_route", "attempt", "commit", "progress"]
    );
}

#[derive(Default)]
struct Telegram(Mutex<Vec<serde_json::Value>>);
impl crate::telegram::TelegramTransport for Telegram {
    fn post(
        &self,
        _url: Url,
        _headers: HeaderMap,
        body: Vec<u8>,
        _timeout: Duration,
    ) -> BoxFuture<'_, Result<HttpReply, TransportError>> {
        self.0
            .lock()
            .unwrap()
            .push(serde_json::from_slice(&body).unwrap());
        Box::pin(async {
            Ok(HttpReply::new(
                StatusCode::OK,
                br#"{"ok":true,"result":{"message_id":12,"chat":{"id":-200}}}"#.to_vec(),
            ))
        })
    }
}
#[tokio::test]
async fn default_native_telegram_targets_frozen_chat_and_thread_without_reply_to_origin() {
    let (mut sender, relay, mut channel, event) = setup();
    let destination = configured(
        &mut channel,
        json!({"type":"telegram_chat", "source_id":"telegram", "chat_id":"-200", "thread_id":"7"}),
    );
    let transport = Arc::new(Telegram::default());
    sender.relay.telegram =
        crate::telegram::TelegramClient::new("https://telegram.example", Duration::from_secs(1))
            .unwrap()
            .with_transport(transport.clone());
    let gate = RecordingGate::new(true);
    let trace = tracing::Span::none();
    let receipt = sender
        .default_reply(
            &SendContext {
                trace: &trace,
                channel: &channel,
                first_send_done: true,
                gate: &gate,
            },
            &event,
            &message("answer".into()),
            &destination,
            "stable-operation",
        )
        .await;
    assert!(matches!(
        receipt.delivery,
        DeliveryOutcome::Confirmed { .. }
    ));
    let sent = transport.0.lock().unwrap();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0]["chat_id"], "-200");
    assert_eq!(sent[0]["message_thread_id"], "7");
    assert!(sent[0].get("reply_to_message_id").is_none());
    assert!(relay.0.lock().unwrap().is_empty());
}

#[derive(Default)]
struct Webhook(Mutex<Vec<(Url, serde_json::Value)>>);
impl crate::delivery::webhook_client::WebhookTransport for Webhook {
    fn resolve(
        &self,
        _url: Url,
    ) -> BoxFuture<'_, Result<Vec<std::net::SocketAddr>, TransportError>> {
        Box::pin(async { Ok(vec!["8.8.8.8:443".parse().unwrap()]) })
    }
    fn post(
        &self,
        url: Url,
        _addresses: Vec<std::net::SocketAddr>,
        _headers: HeaderMap,
        body: Arc<Vec<u8>>,
    ) -> BoxFuture<'_, Result<u16, TransportError>> {
        self.0
            .lock()
            .unwrap()
            .push((url, serde_json::from_slice(&body).unwrap()));
        Box::pin(async { Ok(200) })
    }
}
#[tokio::test]
async fn webhook_destinations_are_nonsecret_and_edits_refuse_rather_than_redirect() {
    let (mut sender, relay, mut channel, event) = setup();
    let destination = configured(&mut channel, json!({"type":"sinks"}));
    let frozen = serde_json::to_string(&destination).unwrap();
    assert!(frozen.contains("https://receiver.example/events"));
    assert!(!frozen.contains("Zml4dHVyZQ=="));
    let transport = Arc::new(Webhook::default());
    sender.custom.webhooks =
        crate::delivery::webhook_client::WebhookClient::with_transport(transport.clone());
    // Key rotation does not change the nonsecret destination authority.
    Arc::get_mut(&mut channel)
        .unwrap()
        .definition
        .secrets
        .hmac_secrets[0]
        .1 = Secret::new("rotated-fixture");
    assert!(destination.authorized(&channel));
    let trace = tracing::Span::none();
    let gate = RecordingGate::new(true);
    let receipt = sender
        .default_reply(
            &SendContext {
                trace: &trace,
                channel: &channel,
                first_send_done: true,
                gate: &gate,
            },
            &event,
            &message("answer".into()),
            &destination,
            "stable-operation",
        )
        .await;
    assert!(matches!(
        receipt.delivery,
        DeliveryOutcome::Confirmed { .. }
    ));
    assert_eq!(transport.0.lock().unwrap().len(), 1);
    assert!(relay.0.lock().unwrap().is_empty());
    let definition = &mut Arc::get_mut(&mut channel).unwrap().definition;
    definition.routing.as_mut().unwrap().default_reply = Some(DefaultReply::TelegramChat {
        source_id: cmaeg_core::channel::SourceId::parse("telegram").unwrap(),
        chat_id: "-200".into(),
        thread_id: None,
    });
    let receipt = sender
        .default_reply(
            &SendContext {
                trace: &trace,
                channel: &channel,
                first_send_done: true,
                gate: &gate,
            },
            &event,
            &message("answer".into()),
            &destination,
            "stable-operation",
        )
        .await;
    assert!(matches!(receipt.delivery, DeliveryOutcome::Rejected { .. }));
    assert_eq!(transport.0.lock().unwrap().len(), 1);
}
