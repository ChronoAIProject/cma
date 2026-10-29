use super::{
    native_lark_media_fixture::{self as media, sent_card},
    *,
};
use crate::interactions::{
    native_lark::{self, fixture::Fixture},
    PreparedCard,
};
use cmaeg_core::{channel::LarkRegion, message::NativeMediaSource};
use reqwest::Method;
use std::sync::atomic::Ordering;

fn message(count: usize, choice: bool) -> OutboundMessage {
    let mut request = media::message(choice, "image/png");
    request.native_attachments = (0..count)
        .map(|index| {
            crate::native_lark::outbound::tests::fixture::attachment(if index % 2 == 0 {
                "image/png"
            } else {
                "image/jpeg"
            })
        })
        .collect();
    request
}
fn image_keys(markup: &serde_json::Value) -> Vec<&str> {
    markup["body"]["elements"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|element| element.get("img_key").and_then(|key| key.as_str()))
        .collect()
}

#[tokio::test]
async fn native_lark_multiple_images_keep_order_custody_edits_and_callback_replay_when_gates_stop()
{
    for region in [LarkRegion::Lark, LarkRegion::Feishu] {
        for count in [2, 4] {
            for named in [false, true] {
                for thread in [false, true] {
                    let f = Fixture::new(named, thread, region).await;
                    let (mut sender, api) = media::sender(&f);
                    sender.relay = sender.relay.with_native_lark_multi_image_controls(true);
                    api.unique_images.store(true, Ordering::SeqCst);
                    let mut request = message(count, named);
                    request.thread_reply = thread;
                    let gate = RecordingGate::new(true);
                    let ctx = SendContext {
                        trace: &tracing::Span::none(),
                        channel: &f.channel,
                        first_send_done: false,
                        gate: &gate,
                    };
                    let sent = sender.reply(&ctx, &f.event, &request).await.0;
                    assert_eq!(sent.delivery.kind(), "confirmed");
                    let reference = sent
                        .message_ref
                        .unwrap()
                        .open(&sender.keys)
                        .unwrap()
                        .payload;
                    let (card, _) = f
                        .store
                        .native_lark_reference(&f.channel, &reference)
                        .await
                        .unwrap();
                    let stored = serde_json::to_value(&card).unwrap();
                    assert!(stored["native_lark"].get("image").is_none());
                    assert_eq!(
                        stored["native_lark"]["images"].as_array().unwrap().len(),
                        count
                    );
                    assert!(card.message.native_attachments.is_empty());
                    let wire = serde_json::to_string(&card).unwrap();
                    assert!(!wire.contains("base64"));
                    assert!(!wire.contains("data:image"));
                    assert!(wire.len() < 16 * 1024);
                    let markup = native_lark::markup(&card).unwrap().0;
                    let expected: Vec<_> = (1..=count)
                        .map(|i| format!("img_confirmed_card_{i}"))
                        .collect();
                    assert_eq!(image_keys(&markup), expected);
                    assert_eq!(sent_card(&api.calls.lock().unwrap()), markup);
                    assert_eq!(
                        gate.calls().iter().filter(|c| **c == "attempt").count(),
                        count + 1
                    );
                    for case in 0..4 {
                        let mut changed = stored.clone();
                        match case {
                            0 => changed["native_lark"]["images"]
                                .as_array_mut()
                                .unwrap()
                                .reverse(),
                            1 => {
                                changed["native_lark"]["images"]
                                    .as_array_mut()
                                    .unwrap()
                                    .pop();
                            }
                            2 => {
                                changed["native_lark"]["images"][0]["key"] = "img_unattested".into()
                            }
                            3 => changed["native_lark"]["image"] = json!({"key":"img_unattested"}),
                            _ => unreachable!(),
                        }
                        let changed: crate::interactions::Card =
                            serde_json::from_value(changed).unwrap();
                        assert!(!changed
                            .native_lark
                            .as_ref()
                            .unwrap()
                            .current(&changed, &f.channel));
                    }
                    // Retained readers and original authority survive every issuance gate stopping.
                    sender.relay = sender
                        .relay
                        .with_native_lark_multi_image_controls(false)
                        .with_native_lark_media_controls(false)
                        .with_native_lark_controls(false, false)
                        .with_native_lark_outbound_media(false);
                    let before = api.calls.lock().unwrap().len();
                    // No image add/replace/remove request is accepted as an edit.
                    assert_eq!(
                        sender
                            .update(&ctx, &reference, &request)
                            .await
                            .0
                            .delivery
                            .kind(),
                        "unsupported"
                    );
                    assert_eq!(api.calls.lock().unwrap().len(), before);
                    request.native_attachments.clear();
                    request.text = Some("Revised text with the original ordered images".into());
                    let edited = sender.update(&ctx, &reference, &request).await.0;
                    assert_eq!(edited.delivery.kind(), "confirmed");
                    let edited_ref = edited
                        .message_ref
                        .unwrap()
                        .open(&sender.keys)
                        .unwrap()
                        .payload;
                    let (next, version) = f
                        .store
                        .native_lark_reference(&f.channel, &edited_ref)
                        .await
                        .unwrap();
                    assert_eq!(next.expires_at, card.expires_at);
                    assert_eq!(next.consumer, card.consumer);
                    assert_eq!(
                        edited_ref.platform_message_id,
                        reference.platform_message_id
                    );
                    let next_markup = native_lark::markup(&next).unwrap().0;
                    assert_eq!(image_keys(&next_markup), expected);
                    let next = PreparedCard {
                        card: next,
                        version,
                        markup: next_markup,
                    };
                    let callback = f.callback(&next, "multi_image_clicked", 0);
                    let id = f
                        .store
                        .consume_native_lark(&f.channel, &callback)
                        .await
                        .unwrap();
                    assert_eq!(
                        f.store
                            .consume_native_lark(&f.channel, &callback)
                            .await
                            .unwrap(),
                        id
                    );
                    let pending = f.store.native_lark_pending(&id, &f.channel).await.unwrap();
                    assert_eq!(pending.event.consumer, f.event.consumer);
                    assert_eq!(pending.event.conversation_id, f.event.conversation_id);
                    assert_eq!(pending.activity.conversation.thread_id.is_some(), thread);
                    native_lark::current_dispatch(
                        &f.server.records,
                        &sender.keys,
                        &f.channel,
                        &pending.activity,
                        &pending.event,
                    )
                    .await
                    .unwrap();
                    assert!(f
                        .store
                        .native_lark_recovery_page(None)
                        .await
                        .unwrap()
                        .cards
                        .iter()
                        .any(|(x, _)| x == &id));
                    f.store.native_lark_admitted(&id).await.unwrap();
                    assert!(!f
                        .store
                        .native_lark_recovery_page(None)
                        .await
                        .unwrap()
                        .cards
                        .iter()
                        .any(|(x, _)| x == &id));
                    let calls = api.calls.lock().unwrap();
                    assert_eq!(
                        calls
                            .iter()
                            .filter(|(_, u, _, _)| u.path().ends_with("/images"))
                            .count(),
                        count
                    );
                    assert_eq!(
                        calls
                            .iter()
                            .filter(|(m, u, _, _)| *m == Method::POST
                                && (u.path().ends_with("/messages")
                                    || u.path().ends_with("/reply")))
                            .count(),
                        1
                    );
                    assert_eq!(
                        calls
                            .iter()
                            .filter(|(m, _, _, _)| *m == Method::PATCH)
                            .count(),
                        1
                    );
                    let patch: serde_json::Value = serde_json::from_slice(
                        &calls
                            .iter()
                            .find(|(m, _, _, _)| *m == Method::PATCH)
                            .unwrap()
                            .3,
                    )
                    .unwrap();
                    let patched: serde_json::Value =
                        serde_json::from_str(patch["content"].as_str().unwrap()).unwrap();
                    assert_eq!(image_keys(&patched), expected);
                }
            }
        }
    }
}

#[tokio::test]
async fn native_lark_multiple_images_validate_all_sources_and_gates_before_any_platform_call() {
    for case in 0..9 {
        let f = Fixture::new(false, false, LarkRegion::Lark).await;
        let (mut sender, api) = media::sender(&f);
        sender.relay = sender.relay.with_native_lark_multi_image_controls(true);
        let mut request = message(4, false);
        match case {
            0 => sender.relay = sender.relay.with_native_lark_multi_image_controls(false),
            1 => sender.relay = sender.relay.with_native_lark_media_controls(false),
            2 => sender.relay = sender.relay.with_native_lark_outbound_media(false),
            3 => {
                request.native_attachments[3].source = NativeMediaSource::Base64 {
                    data: STANDARD.encode(b"invalid image"),
                }
            }
            4 => request.native_attachments[3].caption = Some("unsupported caption".into()),
            5 => {
                request.native_attachments[3] = media::message(false, "text/plain")
                    .native_attachments
                    .remove(0)
            }
            6 => request
                .native_attachments
                .push(request.native_attachments[0].clone()),
            7 => request.blocks = native_lark::fixture::outbound(true).blocks,
            8 => request.text = Some("x".repeat(4001)),
            _ => unreachable!(),
        }
        let gate = RecordingGate::new(true);
        let ctx = SendContext {
            trace: &tracing::Span::none(),
            channel: &f.channel,
            first_send_done: false,
            gate: &gate,
        };
        let sent = sender.reply(&ctx, &f.event, &request).await.0;
        assert!(
            matches!(sent.delivery.kind(), "unsupported" | "rejected"),
            "case {case}"
        );
        assert!(sent.message_ref.is_none());
        assert!(api.calls.lock().unwrap().is_empty(), "case {case}");
    }
}

#[tokio::test]
async fn native_lark_partial_multiple_image_upload_and_lost_send_never_split_or_issue_authority() {
    for fault in [1, 2, 3] {
        let f = Fixture::new(false, false, LarkRegion::Feishu).await;
        let (mut sender, api) = media::sender(&f);
        sender.relay = sender.relay.with_native_lark_multi_image_controls(true);
        api.fault.store(fault, Ordering::SeqCst);
        api.upload_fault_at.store(2, Ordering::SeqCst);
        let gate = RecordingGate::new(true);
        let ctx = SendContext {
            trace: &tracing::Span::none(),
            channel: &f.channel,
            first_send_done: false,
            gate: &gate,
        };
        let sent = sender.reply(&ctx, &f.event, &message(4, false)).await.0;
        assert!(sent.message_ref.is_none());
        assert_eq!(
            sent.delivery.kind(),
            if fault == 2 { "rejected" } else { "unknown" }
        );
        let calls = api.calls.lock().unwrap();
        assert_eq!(
            calls
                .iter()
                .filter(|(_, u, _, _)| u.path().ends_with("/images"))
                .count(),
            if fault < 3 { 2 } else { 4 }
        );
        assert_eq!(
            calls
                .iter()
                .filter(|(m, u, _, _)| *m == Method::POST
                    && (u.path().ends_with("/messages") || u.path().ends_with("/reply")))
                .count(),
            usize::from(fault == 3)
        );
    }
}

#[tokio::test]
async fn native_lark_multiple_images_stop_after_first_upload_when_source_or_bot_is_revoked() {
    for bot_changed in [false, true] {
        let f = Fixture::new(true, true, LarkRegion::Lark).await;
        let (mut sender, api) = media::sender(&f);
        sender.relay = sender.relay.with_native_lark_multi_image_controls(true);
        if bot_changed {
            api.fault.store(6, Ordering::SeqCst);
        } else {
            let memory = f.server.memory.clone();
            let mut definition = f.channel.definition.clone();
            definition.sources.clear();
            *api.after_upload.lock().unwrap() = Some(Box::new(move || {
                memory
                    .channels
                    .lock()
                    .unwrap()
                    .get_mut(definition.record_id.as_str())
                    .unwrap()
                    .blob = cmaeg_core::seal::seal_blob(
                    &crate::ingress::routing_plan::fixture::keys(),
                    &serde_json::to_vec(&definition.to_blob_json()).unwrap(),
                )
                .unwrap();
            }));
        }
        let gate = RecordingGate::new(true);
        let ctx = SendContext {
            trace: &tracing::Span::none(),
            channel: &f.channel,
            first_send_done: false,
            gate: &gate,
        };
        let sent = sender.reply(&ctx, &f.event, &message(4, false)).await.0;
        assert!(sent.message_ref.is_none());
        assert_ne!(sent.delivery.kind(), "confirmed");
        assert_eq!(
            api.calls
                .lock()
                .unwrap()
                .iter()
                .filter(
                    |(m, u, _, _)| *m == Method::POST && !u.path().contains("tenant_access_token")
                )
                .count(),
            1
        );
    }
}

#[tokio::test]
async fn native_lark_multiple_image_failed_patch_is_fenced_without_reupload_or_repeat() {
    let f = Fixture::new(false, false, LarkRegion::Lark).await;
    let (mut sender, api) = media::sender(&f);
    sender.relay = sender.relay.with_native_lark_multi_image_controls(true);
    let gate = RecordingGate::new(true);
    let ctx = SendContext {
        trace: &tracing::Span::none(),
        channel: &f.channel,
        first_send_done: false,
        gate: &gate,
    };
    let sent = sender.reply(&ctx, &f.event, &message(2, false)).await.0;
    let reference = sent
        .message_ref
        .unwrap()
        .open(&sender.keys)
        .unwrap()
        .payload;
    api.fault.store(5, Ordering::SeqCst);
    let update = native_lark::fixture::outbound(false);
    let edited = sender.update(&ctx, &reference, &update).await.0;
    assert_eq!(edited.delivery.kind(), "unknown");
    assert!(edited.message_ref.is_none());
    assert!(sender
        .update(&ctx, &reference, &update)
        .await
        .0
        .message_ref
        .is_none());
    let calls = api.calls.lock().unwrap();
    assert_eq!(
        calls
            .iter()
            .filter(|(_, u, _, _)| u.path().ends_with("/images"))
            .count(),
        2
    );
    assert_eq!(
        calls
            .iter()
            .filter(|(m, _, _, _)| *m == Method::PATCH)
            .count(),
        1
    );
}
