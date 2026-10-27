use super::native_whatsapp_tests::{media_setup, send_media, RevokeAfterUpload};
use super::*;
use crate::whatsapp::{
    self,
    outbound::tests::{answer, attachment, confirmed},
};
use cmaeg_core::message::{NativeMediaKind, NativeMediaSource};

#[tokio::test]
async fn native_whatsapp_video_alone_and_mixed_batches_preserve_captions_target_and_receipts() {
    for (mime, _) in whatsapp::outbound::video::tests::fixtures() {
        let mixed = mime == "video/3gpp";
        let mut replies = Vec::new();
        if mixed {
            replies.push(confirmed("wamid.text"));
        }
        replies.extend([answer(json!({"id":"45678"})), confirmed("wamid.video")]);
        if mixed {
            replies.extend([
                answer(json!({"id":"45679"})),
                confirmed("wamid.image"),
                answer(json!({"id":"45680"})),
                confirmed("wamid.audio"),
            ]);
        }
        let (sender, channel, event, graph, _server) = media_setup(replies).await;
        let mut message = message("Explanation".into());
        if !mixed {
            message.text = None;
        }
        message.native_attachments = vec![attachment(mime)];
        if mixed {
            message
                .native_attachments
                .extend([attachment("image/png"), attachment("audio/mp4")]);
        }
        let gate = RecordingGate::new(true);
        let receipt = send_media(&sender, &channel, &event, &message, &gate).await;
        assert!(matches!(
            receipt.delivery,
            DeliveryOutcome::Confirmed { .. }
        ));
        assert!(receipt.message_ref.is_none());
        {
            let calls = graph.calls.lock().unwrap();
            assert_eq!(calls.len(), if mixed { 7 } else { 2 });
            assert_eq!(
                gate.calls()
                    .iter()
                    .filter(|call| **call == "attempt")
                    .count(),
                calls.len()
            );
            let body: serde_json::Value =
                serde_json::from_slice(&calls[if mixed { 2 } else { 1 }].2).unwrap();
            assert_eq!(body["type"], "video");
            assert_eq!(body["video"], json!({"id":"45678","caption":"A caption"}));
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
                "wamid.video",
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
async fn native_whatsapp_video_preflights_the_whole_batch_before_text_or_upload() {
    for defect in 0..7 {
        let (sender, channel, event, graph, _server) = media_setup(vec![]).await;
        let mut message = message("Must not escape preflight".into());
        let mut item = attachment("video/mp4");
        match defect {
            0 => item.kind = NativeMediaKind::File,
            1 => item.mime_type = Some("video/3gpp".into()),
            2 => item.filename = Some("wrong.3gp".into()),
            3 => {
                item.source = NativeMediaSource::Base64 {
                    data: STANDARD.encode(b"\0\0\0\x18ftypmp42\0\0\0\0mp42isom"),
                }
            }
            4 => {
                item.source = NativeMediaSource::Base64 {
                    data: STANDARD
                        .encode(include_bytes!("../../../tests/fixtures/media/h264_aac.mp4")),
                }
            }
            5 => {
                let bytes = whatsapp::outbound::video::tests::fixtures()[0].1;
                item.source = NativeMediaSource::Base64 {
                    data: STANDARD.encode(&bytes[..bytes.len() - 1]),
                };
            }
            6 => item.caption = Some("x".repeat(1025)),
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
async fn native_whatsapp_video_uncertainty_receipt_loss_and_revocation_stop_remaining_uploads() {
    for failure in 0..4 {
        let replies = match failure {
            0 => vec![Err(TransportError::Unavailable)],
            1 => vec![
                answer(json!({"id":"45678"})),
                Err(TransportError::Unavailable),
            ],
            2 => vec![answer(json!({"id":"45678"}))],
            _ => vec![answer(json!({"id":"45678"})), confirmed("wamid.video")],
        };
        let (mut sender, channel, event, graph, _server) = media_setup(replies).await;
        if failure == 3 {
            sender.relay.whatsapp_receipts = Some(Arc::new(whatsapp::receipts::Store::new(
                crate::records::RecordClient::new("http://127.0.0.1:1", "fixture").unwrap(),
                sender.keys.clone(),
            )));
        }
        let mut message = message(String::new());
        message.text = None;
        message.native_attachments = vec![attachment("video/mp4"), attachment("application/pdf")];
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
            if matches!(failure, 1 | 3) { 2 } else { 1 }
        );
    }
}
