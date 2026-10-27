use super::native_whatsapp_tests::{media_setup, send_media, RevokeAfterUpload};
use super::*;
use crate::whatsapp::{
    self,
    outbound::tests::{answer, attachment, confirmed},
};
use cmaeg_core::message::{NativeMediaKind, NativeMediaSource};

#[tokio::test]
async fn native_whatsapp_sticker_static_and_animated_messages_preserve_receipts_and_exact_target() {
    for (_, bytes, animated) in whatsapp::outbound::sticker::tests::fixtures() {
        let mut replies = Vec::new();
        if animated {
            replies.push(confirmed("wamid.text"));
        }
        replies.extend([answer(json!({"id":"45678"})), confirmed("wamid.sticker")]);
        let (sender, channel, event, graph, _server) = media_setup(replies).await;
        let mut message = message("Separate text".into());
        if !animated {
            message.text = None;
        }
        let mut item = attachment("image/webp");
        item.source = NativeMediaSource::Base64 {
            data: STANDARD.encode(bytes),
        };
        message.native_attachments = vec![item];
        let receipt = send_media(
            &sender,
            &channel,
            &event,
            &message,
            &RecordingGate::new(true),
        )
        .await;
        assert!(matches!(
            receipt.delivery,
            DeliveryOutcome::Confirmed { .. }
        ));
        assert!(receipt.message_ref.is_none());
        {
            let calls = graph.calls.lock().unwrap();
            assert_eq!(calls.len(), if animated { 3 } else { 2 });
            let body: serde_json::Value = serde_json::from_slice(&calls.last().unwrap().2).unwrap();
            assert_eq!(body["type"], "sticker");
            assert_eq!(body["sticker"], json!({"id":"45678"}));
            assert_eq!(body["to"], "447700900123");
            assert_eq!(body["context"]["message_id"], "wamid.original");
        }
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
                "wamid.sticker",
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

#[tokio::test]
async fn native_whatsapp_sticker_preflights_the_whole_batch_before_any_effect() {
    for defect in 0..6 {
        let (sender, channel, event, graph, _server) = media_setup(vec![]).await;
        let mut message = message("Text must not escape preflight".into());
        let mut item = attachment("image/webp");
        match defect {
            0 => item.caption = Some("Unsupported caption".into()),
            1 => item.kind = NativeMediaKind::Image,
            2 => item.mime_type = Some("image/png".into()),
            3 => item.filename = Some("wrong.png".into()),
            4 => {
                item.source = NativeMediaSource::Base64 {
                    data: STANDARD.encode(b"RIFF\0\0\0\0WEBP"),
                }
            }
            5 => {
                let mut bytes = whatsapp::outbound::sticker::tests::fixtures()[1].1.to_vec();
                bytes[24] = 0xfe; // wrong canvas width, while keeping the container intact.
                item.source = NativeMediaSource::Base64 {
                    data: STANDARD.encode(bytes),
                };
            }
            _ => unreachable!(),
        }
        message.native_attachments = vec![attachment("application/pdf"), item];
        let receipt = send_media(
            &sender,
            &channel,
            &event,
            &message,
            &RecordingGate::new(true),
        )
        .await;
        assert!(matches!(
            receipt.delivery,
            DeliveryOutcome::Rejected { .. } | DeliveryOutcome::Unsupported
        ));
        assert!(graph.calls.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn native_whatsapp_sticker_unknown_upload_send_and_revocation_stop_the_batch() {
    for failure in 0..3 {
        let replies = match failure {
            0 => vec![Err(TransportError::Unavailable)],
            1 => vec![
                answer(json!({"id":"45678"})),
                Err(TransportError::Unavailable),
            ],
            _ => vec![answer(json!({"id":"45678"}))],
        };
        let (sender, channel, event, graph, _server) = media_setup(replies).await;
        let mut message = message(String::new());
        message.text = None;
        message.native_attachments = vec![attachment("image/webp"), attachment("application/pdf")];
        let allowed = RecordingGate::new(true);
        let revoked = RevokeAfterUpload(std::sync::atomic::AtomicUsize::new(0));
        let gate: &dyn crate::delivery::gate::SendGate =
            if failure == 2 { &revoked } else { &allowed };
        let receipt = send_media(&sender, &channel, &event, &message, gate).await;
        assert!(matches!(
            receipt.delivery,
            DeliveryOutcome::Unknown { .. } | DeliveryOutcome::Rejected { .. }
        ));
        assert_eq!(
            graph.calls.lock().unwrap().len(),
            if failure == 1 { 2 } else { 1 }
        );
    }
}

#[tokio::test]
async fn sticker_requests_are_refused_before_native_or_relay_transports_other_than_whatsapp() {
    let (mut sender, relay, channel, mut event) = setup();
    sender.relay = sender.relay.with_native_media(true);
    let mut message = message("Must not send".into());
    message.native_attachments = vec![attachment("image/webp")];
    let trace = tracing::Span::none();
    let gate = RecordingGate::new(true);
    let ctx = SendContext {
        trace: &trace,
        channel: &channel,
        first_send_done: false,
        gate: &gate,
    };
    // Existing setup's NyxID route and a native Discord route both refuse before
    // credentials, uploads or any transport-specific side effect can be reached.
    let (receipt, _) = sender.reply(&ctx, &event, &message).await;
    assert!(matches!(receipt.delivery, DeliveryOutcome::Unsupported));
    assert!(
        matches!(RelayClient::prepare_native(&event.reply, "Must not send", &message.native_attachments),
        Err(DeliveryOutcome::Rejected { reason }) if reason == "native_media_unsupported")
    );
    assert!(
        matches!(sender.relay.prepare_initiated(Platform::Whatsapp, "route-fixture", "fixture-key", &message),
        Err(problem) if problem.detail.as_deref() == Some("native_media_unsupported"))
    );
    event.reply = ReplyAddress::Discord {
        bot_id: "12345".into(),
        channel_id: "34567".into(),
        message_id: "45678".into(),
        thread_id: None,
    };
    let (receipt, _) = sender.reply(&ctx, &event, &message).await;
    assert!(matches!(receipt.delivery, DeliveryOutcome::Unsupported));
    event.reply = ReplyAddress::Telegram {
        bot_id: "12345".into(),
        chat_id: "-42".into(),
        message_id: "123".into(),
        thread_id: None,
    };
    let (receipt, _) = sender.reply(&ctx, &event, &message).await;
    assert!(matches!(receipt.delivery, DeliveryOutcome::Unsupported));
    assert!(relay.0.lock().unwrap().is_empty());
    assert!(gate.calls().is_empty());
}
