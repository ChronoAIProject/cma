use super::{
    native_whatsapp_control_tests::{sender, Graph},
    native_whatsapp_tests::{send_media, RevokeAfterUpload},
    *,
};
use crate::{
    interactions::whatsapp_native_fixture::{message as controls, Fixture},
    whatsapp::outbound::{tests::attachment, MIME_TYPES},
};
use std::sync::atomic::{AtomicUsize, Ordering};

fn media_message(mime: &str) -> OutboundMessage {
    let mut message = controls(false);
    let mut item = attachment(mime);
    item.caption = None;
    message.native_attachments.push(item);
    message
}
fn media_sender(f: &Fixture, mode: usize) -> (Sender, Arc<Graph>) {
    let (mut sender, graph) = sender(f, mode, true);
    sender.relay = sender
        .relay
        .with_native_whatsapp_outbound_media(true)
        .with_native_whatsapp_media_controls(true);
    (sender, graph)
}
fn locator(graph: &Graph) -> String {
    graph.seen.lock().unwrap()[0]["interactive"]["action"]["buttons"][0]["reply"]["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[tokio::test]
async fn native_whatsapp_media_controls_bind_one_header_and_replay_original_callbacks() {
    for named in [false, true] {
        for mime in MIME_TYPES
            .into_iter()
            .filter(|mime| !mime.starts_with("audio/") && *mime != "image/webp")
        {
            let f = Fixture::new(named).await;
            let (mut sender, graph) = media_sender(&f, 0);
            let message = media_message(mime);
            let gate = RecordingGate::new(true);
            let receipt = send_media(&sender, &f.channel, &f.event, &message, &gate).await;
            assert!(
                matches!(receipt.delivery, DeliveryOutcome::Confirmed { .. }),
                "{mime}"
            );
            let reference = receipt
                .message_ref
                .unwrap()
                .open(&sender.keys)
                .unwrap()
                .payload;
            assert_eq!(reference.consumer, f.event.consumer);
            assert_eq!(reference.platform_message_id, "wamid.confirmed");
            let card = f
                .store
                .load(reference.interaction_record_id.as_ref().unwrap())
                .await
                .unwrap()
                .0;
            assert_eq!(
                serde_json::to_value(card.message).unwrap(),
                serde_json::to_value(controls(false)).unwrap()
            );
            assert_eq!(card.destination.unwrap().platform_id, "wamid.confirmed");
            assert_eq!(graph.calls.load(Ordering::SeqCst), 2);
            assert_eq!(
                gate.calls(),
                [
                    "enter_route",
                    "commit",
                    "attempt",
                    "progress",
                    "attempt",
                    "progress"
                ]
            );
            {
                let seen = graph.seen.lock().unwrap();
                assert_eq!(seen.len(), 1);
                let body = &seen[0];
                assert_eq!(body["type"], "interactive");
                assert_eq!(body["to"], "447700900123");
                assert_eq!(body["context"]["message_id"], "wamid.original");
                assert_eq!(body["interactive"]["type"], "button");
                assert_eq!(body["interactive"]["body"]["text"], "Choose");
                let kind = if mime.starts_with("image/") {
                    "image"
                } else if mime.starts_with("video/") {
                    "video"
                } else {
                    "document"
                };
                let header = &body["interactive"]["header"];
                assert_eq!(header["type"], kind);
                assert_eq!(header[kind]["id"], "45678");
                assert!(header[kind].get("caption").is_none());
                assert_eq!(header[kind].get("filename").is_some(), kind == "document");
                assert!(!body.to_string().contains("private-"));
            }
            sender.relay = sender.relay.with_native_whatsapp_media_controls(false);
            let refused = send_media(&sender, &f.channel, &f.event, &message, &gate).await;
            assert!(matches!(refused.delivery, DeliveryOutcome::Unsupported));
            assert_eq!(graph.calls.load(Ordering::SeqCst), 2);
            let loc = locator(&graph);
            let activity = f.activity(&loc, "wamid.click");
            let route = crate::interactions::whatsapp_authority::route(
                &activity.source,
                &activity.actor.id,
            )
            .unwrap();
            let consume = || {
                f.store.consume_for_dispatch(
                    &f.channel,
                    &activity,
                    &route,
                    "wamid.click",
                    &loc,
                    "wamid.confirmed",
                    None,
                )
            };
            let first = consume().await.unwrap();
            let writes = f.server.memory.writes.load(Ordering::SeqCst);
            let replay = consume().await.unwrap();
            assert_eq!(
                serde_json::to_value(first.0).unwrap(),
                serde_json::to_value(replay.0).unwrap()
            );
            assert_eq!(first.1, replay.1);
            assert_eq!(f.server.memory.writes.load(Ordering::SeqCst), writes);
            assert_eq!(graph.calls.load(Ordering::SeqCst), 2);
        }
    }
}

#[tokio::test]
async fn native_whatsapp_media_controls_refuse_unsupported_mixtures_and_gates_before_effects() {
    for defect in 0..16 {
        let f = Fixture::new(true).await;
        let (mut sender, graph) = media_sender(&f, 0);
        let mut message = media_message("image/png");
        match defect {
            0 => sender.relay = sender.relay.with_native_whatsapp_media_controls(false),
            1 => sender.relay = sender.relay.with_native_whatsapp_outbound_media(false),
            2 => sender.relay = sender.relay.with_native_whatsapp_interactions(false),
            3 => sender.relay = sender.relay.with_consumer_interactions(false),
            4 => message.blocks = controls(true).blocks,
            5 => message.native_attachments.push(message.native_attachments[0].clone()),
            6 => message.native_attachments[0] = attachment("audio/mpeg"),
            7 => message.native_attachments[0] = attachment("image/webp"),
            8 => message.native_attachments[0].caption = Some("Unsupported caption".into()),
            9 => message.native_attachments[0].mime_type = Some("image/jpeg".into()),
            10 => message.native_attachments[0].filename = Some("wrong.pdf".into()),
            11 => message.thread_reply = true,
            12 => message.text = Some("x".repeat(1025)),
            13 => message.blocks.push(controls(false).blocks.remove(0)),
            14 => message.native_attachments[0].source = cmaeg_core::message::NativeMediaSource::Base64 { data: STANDARD.encode(b"invalid image") },
            15 => message.blocks.push(serde_json::from_value(json!({"type":"form","id":"form","fields":[{"id":"reason","label":"Reason","required":true,"kind":{"type":"text"}}]})).unwrap()),
            _ => unreachable!(),
        }
        let writes = f.server.memory.writes.load(Ordering::SeqCst);
        let gate = RecordingGate::new(true);
        let receipt = send_media(&sender, &f.channel, &f.event, &message, &gate).await;
        assert!(
            matches!(
                receipt.delivery,
                DeliveryOutcome::Unsupported | DeliveryOutcome::Rejected { .. }
            ),
            "defect {defect}"
        );
        assert!(receipt.message_ref.is_none());
        assert_eq!(graph.calls.load(Ordering::SeqCst), 0, "defect {defect}");
        assert_eq!(f.server.memory.writes.load(Ordering::SeqCst), writes);
        assert!(gate.calls().is_empty());
    }
}

#[tokio::test]
async fn native_whatsapp_media_controls_uncertain_upload_send_and_binding_never_resend() {
    for mode in [1, 2, 3, 6, 7, 8, 9] {
        let f = Fixture::new(false).await;
        let (sender, graph) = media_sender(&f, mode);
        let receipt = send_media(
            &sender,
            &f.channel,
            &f.event,
            &media_message("image/png"),
            &RecordingGate::new(true),
        )
        .await;
        assert!(
            matches!(
                receipt.delivery,
                DeliveryOutcome::Unknown { .. }
                    | DeliveryOutcome::RateLimited { .. }
                    | DeliveryOutcome::Rejected { .. }
            ),
            "mode {mode}"
        );
        assert!(receipt.message_ref.is_none());
        assert_eq!(
            graph.calls.load(Ordering::SeqCst),
            if mode >= 6 { 1 } else { 2 }
        );
        if mode < 6 {
            let loc = locator(&graph);
            let activity = f.activity(&loc, "wamid.click");
            let route = crate::interactions::whatsapp_authority::route(
                &activity.source,
                &activity.actor.id,
            )
            .unwrap();
            assert!(f
                .store
                .consume_for_dispatch(
                    &f.channel,
                    &activity,
                    &route,
                    "wamid.click",
                    &loc,
                    "wamid.confirmed",
                    None
                )
                .await
                .is_err());
        }
    }
}

#[tokio::test]
async fn native_whatsapp_media_controls_recheck_authority_between_upload_and_send() {
    let f = Fixture::new(false).await;
    let (sender, graph) = media_sender(&f, 0);
    let gate = RevokeAfterUpload(AtomicUsize::new(0));
    let receipt = send_media(
        &sender,
        &f.channel,
        &f.event,
        &media_message("image/png"),
        &gate,
    )
    .await;
    assert!(matches!(receipt.delivery, DeliveryOutcome::Rejected { .. }));
    assert!(receipt.message_ref.is_none());
    assert_eq!(graph.calls.load(Ordering::SeqCst), 1);
    assert!(graph.seen.lock().unwrap().is_empty());
}
