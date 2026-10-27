use super::*;
use crate::{
    ingress::routing_store::test_server::Server,
    whatsapp::{self, receipts::Store},
};
use reqwest::Method;

struct Graph(Mutex<Vec<serde_json::Value>>);
impl whatsapp::Transport for Graph {
    fn exchange(
        &self,
        method: Method,
        url: Url,
        _: HeaderMap,
        body: Vec<u8>,
        _: Duration,
    ) -> BoxFuture<'_, Result<HttpReply, TransportError>> {
        assert_eq!(method, Method::POST);
        assert_eq!(url.path(), "/v25.0/34567/messages");
        self.0
            .lock()
            .unwrap()
            .push(serde_json::from_slice(&body).unwrap());
        Box::pin(async {
            Ok(HttpReply::new(StatusCode::OK, serde_json::to_vec(&json!({"messaging_product":"whatsapp","contacts":[{"wa_id":"447700900123"}],"messages":[{"id":"wamid.confirmed"}]})).unwrap()))
        })
    }
}

#[tokio::test]
async fn native_whatsapp_delivery_requires_current_phone_and_persists_confirmed_receipt() {
    for revoked in [false, true] {
        let (mut sender, relay, mut channel, mut event) = setup();
        let server = Server::start_realtime().await;
        let store = Arc::new(Store::new(server.records.clone(), sender.keys.clone()));
        let graph = Arc::new(Graph(Mutex::default()));
        sender.relay = sender.relay.with_whatsapp(
            whatsapp::Client::production()
                .unwrap()
                .with_transport(graph.clone()),
            store.clone(),
        );
        let definition = &mut Arc::make_mut(&mut channel).definition;
        definition.sources = if revoked {
            vec![]
        } else {
            vec![whatsapp::tests::source()]
        };
        definition.secrets.whatsapp = Some(whatsapp::tests::credentials());
        event.reply = ReplyAddress::Whatsapp {
            app_id: "12345".into(),
            waba_id: "23456".into(),
            phone_number_id: "34567".into(),
            recipient_id: "447700900123".into(),
            message_id: "wamid.original".into(),
        };
        let gate = RecordingGate::new(true);
        let trace = tracing::Span::none();
        let ctx = SendContext {
            trace: &trace,
            channel: &channel,
            first_send_done: false,
            gate: &gate,
        };
        let (receipt, _) = sender
            .reply(&ctx, &event, &message("native reply".into()))
            .await;
        if revoked {
            assert!(matches!(receipt.delivery, DeliveryOutcome::Rejected { .. }));
            assert!(graph.0.lock().unwrap().is_empty());
            assert!(gate.calls().is_empty());
        } else {
            assert!(matches!(
                receipt.delivery,
                DeliveryOutcome::Confirmed { .. }
            ));
            let source = cmaeg_core::activity::SourceKind::Whatsapp {
                app_id: "12345".into(),
                waba_id: "23456".into(),
                phone_number_id: "34567".into(),
            };
            let receipt = store
                .load(
                    &event.channel_id,
                    &source,
                    "447700900123",
                    "wamid.confirmed",
                    time::OffsetDateTime::now_utc().unix_timestamp(),
                )
                .await
                .unwrap()
                .unwrap();
            assert_eq!(receipt.original.event_id, event.event_id);
            assert_eq!(receipt.partition_key, event.partition_key);
            assert_eq!(receipt.conversation_id, event.conversation_id);
            assert_eq!(receipt.consumer.as_str(), "default");
            assert_eq!(graph.0.lock().unwrap().len(), 1);
            assert_eq!(
                gate.calls(),
                ["enter_route", "attempt", "commit", "progress"]
            );
        }
        assert!(relay.0.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn native_whatsapp_delivery_never_resends_after_receipt_storage_loss() {
    let (mut sender, _, mut channel, mut event) = setup();
    let graph = Arc::new(Graph(Mutex::default()));
    let records = crate::records::RecordClient::new("http://127.0.0.1:1", "fixture").unwrap();
    sender.relay = sender.relay.with_whatsapp(
        whatsapp::Client::production()
            .unwrap()
            .with_transport(graph.clone()),
        Arc::new(Store::new(records, sender.keys.clone())),
    );
    let definition = &mut Arc::make_mut(&mut channel).definition;
    definition.sources = vec![whatsapp::tests::source()];
    definition.secrets.whatsapp = Some(whatsapp::tests::credentials());
    event.reply = ReplyAddress::Whatsapp {
        app_id: "12345".into(),
        waba_id: "23456".into(),
        phone_number_id: "34567".into(),
        recipient_id: "447700900123".into(),
        message_id: "wamid.original".into(),
    };
    let gate = RecordingGate::new(true);
    let trace = tracing::Span::none();
    let ctx = SendContext {
        trace: &trace,
        channel: &channel,
        first_send_done: false,
        gate: &gate,
    };
    let (receipt, _) = sender
        .reply(&ctx, &event, &message("native reply".into()))
        .await;
    assert!(matches!(receipt.delivery, DeliveryOutcome::Unknown { .. }));
    assert_eq!(graph.0.lock().unwrap().len(), 1);
}

use crate::whatsapp::outbound::tests::{answer, attachment, confirmed, Graph as MediaGraph};

async fn media_setup(
    replies: Vec<Result<HttpReply, TransportError>>,
) -> (
    Sender,
    Arc<ResolvedChannel>,
    EventRefPayload,
    Arc<MediaGraph>,
    Server,
) {
    let (mut sender, _, mut channel, mut event) = setup();
    let server = Server::start_realtime().await;
    let graph = MediaGraph::new(replies);
    sender.relay = sender
        .relay
        .with_whatsapp(
            whatsapp::Client::production()
                .unwrap()
                .with_transport(graph.clone()),
            Arc::new(Store::new(server.records.clone(), sender.keys.clone())),
        )
        .with_native_whatsapp_outbound_media(true);
    let definition = &mut Arc::make_mut(&mut channel).definition;
    definition.sources = vec![whatsapp::tests::source()];
    definition.secrets.whatsapp = Some(whatsapp::tests::credentials());
    event.reply = ReplyAddress::Whatsapp {
        app_id: "12345".into(),
        waba_id: "23456".into(),
        phone_number_id: "34567".into(),
        recipient_id: "447700900123".into(),
        message_id: "wamid.original".into(),
    };
    (sender, channel, event, graph, server)
}
fn media_message() -> OutboundMessage {
    let mut message = message("Explanation".into());
    message.native_attachments = vec![attachment("application/pdf")];
    message
}
async fn send_media(
    sender: &Sender,
    channel: &Arc<ResolvedChannel>,
    event: &EventRefPayload,
    message: &OutboundMessage,
    gate: &dyn crate::delivery::gate::SendGate,
) -> super::super::intents::IntentReceipt {
    sender
        .reply(
            &SendContext {
                trace: &tracing::Span::none(),
                channel,
                first_send_done: false,
                gate,
            },
            event,
            message,
        )
        .await
        .0
}

#[tokio::test]
async fn native_whatsapp_media_records_each_confirmed_message_and_authorizes_each_write() {
    let (sender, channel, event, graph, _server) = media_setup(vec![
        confirmed("wamid.text"),
        answer(json!({"id":"45678"})),
        confirmed("wamid.pdf"),
    ])
    .await;
    let gate = RecordingGate::new(true);
    let receipt = send_media(&sender, &channel, &event, &media_message(), &gate).await;
    assert!(matches!(
        receipt.delivery,
        DeliveryOutcome::Confirmed { .. }
    ));
    assert!(receipt.message_ref.is_none());
    assert_eq!(
        gate.calls(),
        [
            "enter_route",
            "attempt",
            "commit",
            "progress",
            "attempt",
            "commit",
            "progress",
            "attempt",
            "commit",
            "progress"
        ]
    );
    assert_eq!(graph.calls.lock().unwrap().len(), 3);
    for id in ["wamid.text", "wamid.pdf"] {
        let stored = sender
            .relay
            .whatsapp_receipts
            .as_ref()
            .unwrap()
            .load(
                &event.channel_id,
                &cmaeg_core::activity::SourceKind::Whatsapp {
                    app_id: "12345".into(),
                    waba_id: "23456".into(),
                    phone_number_id: "34567".into(),
                },
                "447700900123",
                id,
                time::OffsetDateTime::now_utc().unix_timestamp(),
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.original.event_id, event.event_id);
        assert_eq!(stored.conversation_id, event.conversation_id);
        assert_eq!(stored.partition_key, event.partition_key);
        assert_eq!(stored.consumer.as_str(), "default");
    }
}

#[tokio::test]
async fn native_whatsapp_media_preflight_gate_and_revocation_refuse_before_any_write() {
    for defect in 0..7 {
        let (mut sender, mut channel, event, graph, _server) = media_setup(vec![]).await;
        let mut message = media_message();
        let gate = RecordingGate::new(defect != 6);
        match defect {
            0 => sender.relay = sender.relay.with_native_whatsapp_outbound_media(false),
            1 => Arc::make_mut(&mut channel).definition.sources.clear(),
            2 => Arc::make_mut(&mut channel).definition.secrets.whatsapp = None,
            3 => message.native_attachments.push(attachment("audio/mpeg")),
            4 => message.native_attachments[0].filename = Some("wrong.exe".into()),
            5 => message.thread_reply = true,
            6 => {}
            _ => unreachable!(),
        }
        let receipt = send_media(&sender, &channel, &event, &message, &gate).await;
        assert!(
            matches!(
                receipt.delivery,
                DeliveryOutcome::Unsupported | DeliveryOutcome::Rejected { .. }
            ),
            "defect {defect}"
        );
        assert!(graph.calls.lock().unwrap().is_empty());
        assert!(receipt.message_ref.is_none());
    }
}

#[tokio::test]
async fn native_whatsapp_media_only_has_no_placeholder_and_unknown_upload_or_send_stops_batch() {
    for at_upload in [true, false] {
        let mut replies = Vec::new();
        if !at_upload {
            replies.push(answer(json!({"id":"45678"})));
        }
        replies.push(Err(TransportError::Unavailable));
        let (sender, channel, event, graph, _server) = media_setup(replies).await;
        let mut message = media_message();
        message.text = None;
        message.native_attachments.push(attachment("image/png"));
        let receipt = send_media(
            &sender,
            &channel,
            &event,
            &message,
            &RecordingGate::new(true),
        )
        .await;
        assert!(matches!(receipt.delivery, DeliveryOutcome::Unknown { .. }));
        assert!(receipt.message_ref.is_none());
        let calls = graph.calls.lock().unwrap();
        assert_eq!(calls.len(), if at_upload { 1 } else { 2 });
        assert!(calls[0].0.path().ends_with("/media"));
    }
}

#[tokio::test]
async fn native_whatsapp_media_partial_delivery_and_receipt_loss_stop_remaining_uploads() {
    for receipt_loss in [false, true] {
        let replies = if receipt_loss {
            vec![answer(json!({"id":"45678"})), confirmed("wamid.pdf")]
        } else {
            vec![
                confirmed("wamid.text"),
                Ok(HttpReply::new(
                    StatusCode::BAD_REQUEST,
                    b"{\"error\":{\"code\":131053}}".to_vec(),
                )),
            ]
        };
        let (mut sender, channel, event, graph, _server) = media_setup(replies).await;
        let mut message = media_message();
        if receipt_loss {
            message.text = None;
            sender.relay.whatsapp_receipts = Some(Arc::new(Store::new(
                crate::records::RecordClient::new("http://127.0.0.1:1", "fixture").unwrap(),
                sender.keys.clone(),
            )));
        }
        message.native_attachments.push(attachment("image/png"));
        let receipt = send_media(
            &sender,
            &channel,
            &event,
            &message,
            &RecordingGate::new(true),
        )
        .await;
        assert!(matches!(receipt.delivery, DeliveryOutcome::Unknown { .. }));
        assert!(receipt.message_ref.is_none());
        assert_eq!(graph.calls.lock().unwrap().len(), 2);
    }
}

struct RevokeAfterUpload(std::sync::atomic::AtomicUsize);
impl crate::delivery::gate::SendGate for RevokeAfterUpload {
    fn enter_route<'a>(
        &'a self,
        _: &'a str,
    ) -> BoxFuture<'a, Result<crate::delivery::gate::RouteTicket, crate::delivery::gate::GateRefusal>>
    {
        Box::pin(async { Ok(crate::delivery::gate::RouteTicket::empty()) })
    }
    fn attempt<'a>(
        &'a self,
    ) -> BoxFuture<
        'a,
        Result<crate::delivery::gate::AttemptPermit, crate::delivery::gate::GateRefusal>,
    > {
        Box::pin(async move {
            if self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                Ok(crate::delivery::gate::AttemptPermit::empty())
            } else {
                Err(crate::delivery::gate::GateRefusal {
                    reason: "source_or_credential_revision_changed",
                    retry_after: None,
                })
            }
        })
    }
    fn commit(&self) -> bool {
        true
    }
    fn progress(&self) {}
}
#[tokio::test]
async fn native_whatsapp_media_revocation_after_upload_blocks_recipient_send() {
    let (sender, channel, event, graph, _server) =
        media_setup(vec![answer(json!({"id":"45678"}))]).await;
    let mut message = media_message();
    message.text = None;
    let receipt = send_media(
        &sender,
        &channel,
        &event,
        &message,
        &RevokeAfterUpload(std::sync::atomic::AtomicUsize::new(0)),
    )
    .await;
    assert!(matches!(receipt.delivery, DeliveryOutcome::Rejected { .. }));
    assert_eq!(graph.calls.lock().unwrap().len(), 1);
}
