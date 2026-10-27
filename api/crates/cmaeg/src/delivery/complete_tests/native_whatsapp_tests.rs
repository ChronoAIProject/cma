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

pub(super) async fn media_setup(
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
pub(super) async fn send_media(
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
async fn native_whatsapp_audio_only_and_mixed_media_preserve_targets_and_receipts() {
    for (mime, _) in whatsapp::outbound::audio::tests::fixtures() {
        for mixed in [false, true] {
            // Every codec traverses the composed sender; one mixed batch proves
            // ordering without repeating its real three-second pacing per codec.
            if mixed && mime != "audio/mp4" {
                continue;
            }
            let mut replies = Vec::new();
            let mut message = media_message();
            message.native_attachments = vec![attachment(mime)];
            let mut expected = vec![("audio", "wamid.audio")];
            if mixed {
                replies.push(confirmed("wamid.text"));
                message.native_attachments.push(attachment("image/png"));
                message
                    .native_attachments
                    .push(attachment("application/pdf"));
                expected.extend([("image", "wamid.image"), ("document", "wamid.pdf")]);
            } else {
                message.text = None;
            }
            for (_, id) in &expected {
                replies.push(answer(json!({"id":"45678"})));
                replies.push(confirmed(id));
            }
            let (sender, channel, event, graph, _server) = media_setup(replies).await;
            let gate = RecordingGate::new(true);
            let receipt = send_media(&sender, &channel, &event, &message, &gate).await;
            assert!(
                matches!(receipt.delivery, DeliveryOutcome::Confirmed { .. }),
                "{mime}, mixed={mixed}"
            );
            assert!(receipt.message_ref.is_none());
            let writes = expected.len() * 2 + usize::from(mixed);
            assert_eq!(
                gate.calls()
                    .iter()
                    .filter(|value| **value == "commit")
                    .count(),
                writes
            );
            {
                let calls = graph.calls.lock().unwrap();
                assert_eq!(calls.len(), writes);
                for (index, (kind, _)) in expected.iter().enumerate() {
                    let send = &calls[usize::from(mixed) + index * 2 + 1];
                    let body: serde_json::Value = serde_json::from_slice(&send.2).unwrap();
                    assert_eq!(body["to"], "447700900123");
                    assert_eq!(body["context"]["message_id"], "wamid.original");
                    assert_eq!(body["type"], *kind);
                    if *kind == "audio" {
                        assert_eq!(body["audio"], json!({"id":"45678"}));
                    }
                }
            }
            for (_, id) in expected {
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
            }
        }
    }
}

#[tokio::test]
async fn native_whatsapp_audio_preflights_entire_batch_before_text_or_upload() {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    use cmaeg_core::message::NativeMediaSource;
    for (mime, bytes) in whatsapp::outbound::audio::tests::fixtures() {
        for defect in 0..6 {
            let (sender, channel, event, graph, _server) = media_setup(vec![]).await;
            let mut message = media_message();
            let mut audio = attachment(mime);
            match defect {
                0 => audio.caption = Some("Audio cannot carry a caption".into()),
                1 => audio.mime_type = Some("audio/wav".into()),
                2 => audio.filename = Some("wrong.wav".into()),
                3 => {
                    audio.source = NativeMediaSource::Base64 {
                        data: STANDARD.encode(&bytes[..bytes.len() - 1]),
                    }
                }
                4 => {
                    audio.source = NativeMediaSource::Base64 {
                        data: STANDARD.encode(b"RIFF\0\0\0\0WAVE"),
                    }
                }
                5 => {
                    audio.mime_type = Some("audio/ogg".into());
                    audio.source = NativeMediaSource::Base64 {
                        data: STANDARD.encode(whatsapp::outbound::audio::tests::stereo_ogg()),
                    };
                }
                _ => unreachable!(),
            }
            message.native_attachments.push(audio);
            let receipt = send_media(
                &sender,
                &channel,
                &event,
                &message,
                &RecordingGate::new(true),
            )
            .await;
            assert!(
                matches!(
                    receipt.delivery,
                    DeliveryOutcome::Unsupported | DeliveryOutcome::Rejected { .. }
                ),
                "{mime}, defect={defect}"
            );
            assert!(graph.calls.lock().unwrap().is_empty());
        }
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
            3 => message.native_attachments[0].kind = cmaeg_core::message::NativeMediaKind::Video,
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

pub(super) struct RevokeAfterUpload(pub(super) std::sync::atomic::AtomicUsize);
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

#[tokio::test]
async fn native_whatsapp_audio_uncertainty_receipt_loss_and_revocation_stop_later_effects() {
    for failure in 0..5 {
        let replies = match failure {
            0 => vec![Err(TransportError::Unavailable)],
            1 => vec![
                answer(json!({"id":"45678"})),
                Err(TransportError::Unavailable),
            ],
            2 => vec![answer(json!({"id":"45678"})), confirmed("wamid.audio")],
            3 => vec![answer(json!({"id":"45678"}))],
            4 => vec![
                answer(json!({"id":"45678"})),
                confirmed("wamid.audio"),
                Err(TransportError::Unavailable),
            ],
            _ => unreachable!(),
        };
        let (mut sender, channel, event, graph, _server) = media_setup(replies).await;
        if failure == 2 {
            sender.relay.whatsapp_receipts = Some(Arc::new(Store::new(
                crate::records::RecordClient::new("http://127.0.0.1:1", "fixture").unwrap(),
                sender.keys.clone(),
            )));
        }
        let mut message = media_message();
        message.text = None;
        message.native_attachments = vec![
            attachment("audio/mpeg"),
            attachment("image/png"),
            attachment("application/pdf"),
        ];
        let allowed = RecordingGate::new(true);
        let revoked = RevokeAfterUpload(std::sync::atomic::AtomicUsize::new(0));
        let gate: &dyn crate::delivery::gate::SendGate =
            if failure == 3 { &revoked } else { &allowed };
        let receipt = send_media(&sender, &channel, &event, &message, gate).await;
        if failure == 3 {
            assert!(matches!(receipt.delivery, DeliveryOutcome::Rejected { .. }));
        } else {
            assert!(matches!(receipt.delivery, DeliveryOutcome::Unknown { .. }));
        }
        assert_eq!(
            graph.calls.lock().unwrap().len(),
            match failure {
                0 | 3 => 1,
                4 => 3,
                _ => 2,
            }
        );
        assert!(receipt.message_ref.is_none());
    }
}
