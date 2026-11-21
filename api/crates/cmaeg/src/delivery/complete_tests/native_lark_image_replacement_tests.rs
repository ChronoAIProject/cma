use super::{native_lark_media_fixture as media, *};
use crate::interactions::{
    native_lark::{self, fixture::Fixture},
    PreparedCard,
};
use cmaeg_core::{channel::LarkRegion, message::NativeMediaSource};
use reqwest::Method;
use std::sync::atomic::Ordering;

fn message(count: usize) -> OutboundMessage {
    let mut message = media::message(false, "image/png");
    message.native_attachments = (0..count)
        .map(|index| {
            crate::native_lark::outbound::tests::fixture::attachment(if index % 2 == 0 {
                "image/png"
            } else {
                "image/jpeg"
            })
        })
        .collect();
    message
}
fn images(markup: &serde_json::Value) -> Vec<String> {
    markup["body"]["elements"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|element| element["img_key"].as_str().map(str::to_owned))
        .collect()
}
fn writes(api: &media::Api) -> (usize, usize) {
    let calls = api.calls.lock().unwrap();
    (
        calls
            .iter()
            .filter(|(_, url, _, _)| url.path().ends_with("/images"))
            .count(),
        calls
            .iter()
            .filter(|(method, _, _, _)| *method == Method::PATCH)
            .count(),
    )
}

#[tokio::test]
async fn native_lark_image_replacements_change_counts_preserve_custody_and_confirm_callbacks() {
    for region in [LarkRegion::Lark, LarkRegion::Feishu] {
        for named in [false, true] {
            for (before, after) in [(1, 4), (4, 1), (2, 3)] {
                let f = Fixture::new(named, named, region).await;
                let (mut sender, api) = media::sender(&f);
                sender.relay = sender
                    .relay
                    .with_native_lark_multi_image_controls(true)
                    .with_native_lark_media_card_replacements(true);
                api.unique_images.store(true, Ordering::SeqCst);
                let gate = RecordingGate::new(true);
                let ctx = SendContext {
                    trace: &tracing::Span::none(),
                    channel: &f.channel,
                    first_send_done: false,
                    gate: &gate,
                };
                let sent = sender.reply(&ctx, &f.event, &message(before)).await.0;
                let reference = sent
                    .message_ref
                    .unwrap()
                    .open(&sender.keys)
                    .unwrap()
                    .payload;
                let (old, _) = f
                    .store
                    .native_lark_reference(&f.channel, &reference)
                    .await
                    .unwrap();
                // Existing cards can replace their full bounded set even when
                // new single/multiple-image card issuance has stopped.
                sender.relay = sender
                    .relay
                    .with_native_lark_multi_image_controls(false)
                    .with_native_lark_media_controls(false)
                    .with_native_lark_controls(false, false);
                let mut replacement = message(after);
                replacement.text = Some("New complete image set".into());
                let result = sender.update(&ctx, &reference, &replacement).await.0;
                assert_eq!(result.delivery.kind(), "confirmed");
                let next_ref = result
                    .message_ref
                    .unwrap()
                    .open(&sender.keys)
                    .unwrap()
                    .payload;
                assert_eq!(next_ref.platform_message_id, reference.platform_message_id);
                assert_eq!(next_ref.consumer, reference.consumer);
                let (next, version) = f
                    .store
                    .native_lark_reference(&f.channel, &next_ref)
                    .await
                    .unwrap();
                assert_eq!(next.expires_at, old.expires_at);
                assert_eq!(next.revision, old.revision + 1);
                assert!(next.message.native_attachments.is_empty());
                let value = serde_json::to_value(&next).unwrap();
                assert_eq!(value["native_lark"].get("image").is_some(), after == 1);
                assert_eq!(value["native_lark"].get("images").is_some(), after > 1);
                let markup = native_lark::markup(&next).unwrap().0;
                let expected: Vec<_> = (before + 1..=before + after)
                    .map(|index| format!("img_confirmed_card_{index}"))
                    .collect();
                assert_eq!(images(&markup), expected);
                assert_eq!(writes(&api), (before + after, 1));
                let old_stored = f.store.load(&old.id).await.unwrap().0;
                assert!(old_stored.superseded);
                assert_eq!(
                    images(&native_lark::markup(&old_stored).unwrap().0),
                    images(&native_lark::markup(&old).unwrap().0)
                );
                let next = PreparedCard {
                    card: next,
                    version,
                    markup,
                };
                let callback = f.callback(&next, "replacement-click", 0);
                let id = f
                    .store
                    .consume_native_lark(&f.channel, &callback)
                    .await
                    .unwrap();
                let pending = f.store.native_lark_pending(&id, &f.channel).await.unwrap();
                assert_eq!(pending.event.conversation_id, f.event.conversation_id);
                assert_eq!(pending.event.consumer, f.event.consumer);
                native_lark::current_dispatch(
                    &f.server.records,
                    &sender.keys,
                    &f.channel,
                    &pending.activity,
                    &pending.event,
                )
                .await
                .unwrap();
            }
        }
    }
}

#[tokio::test]
async fn native_lark_replacement_preflight_gate_and_partial_upload_preserve_original_card() {
    for case in 0..6 {
        let f = Fixture::new(false, false, LarkRegion::Feishu).await;
        let (mut sender, api) = media::sender(&f);
        sender.relay = sender
            .relay
            .with_native_lark_media_card_replacements(case != 0);
        let gate = RecordingGate::new(true);
        let ctx = SendContext {
            trace: &tracing::Span::none(),
            channel: &f.channel,
            first_send_done: false,
            gate: &gate,
        };
        let sent = sender.reply(&ctx, &f.event, &message(1)).await.0;
        let reference = sent
            .message_ref
            .unwrap()
            .open(&sender.keys)
            .unwrap()
            .payload;
        let (old, _) = f
            .store
            .native_lark_reference(&f.channel, &reference)
            .await
            .unwrap();
        let original = serde_json::to_value(&old).unwrap();
        let mut replacement = message(if case == 1 { 5 } else { 4 });
        match case {
            2 => {
                replacement.native_attachments[3].source = NativeMediaSource::Base64 {
                    data: "bm90LWEtcG5n".into(),
                }
            }
            3 => replacement.native_attachments[0].caption = Some("unsupported".into()),
            4 | 5 => {
                api.upload_fault_at.store(3, Ordering::SeqCst);
                api.fault
                    .store(if case == 4 { 2 } else { 1 }, Ordering::SeqCst);
            }
            _ => {}
        }
        let result = sender.update(&ctx, &reference, &replacement).await.0;
        assert_ne!(result.delivery.kind(), "confirmed");
        assert!(result.message_ref.is_none());
        assert_eq!(writes(&api), (if case >= 4 { 3 } else { 1 }, 0));
        assert_eq!(
            serde_json::to_value(f.store.load(&old.id).await.unwrap().0).unwrap(),
            original
        );
        // Upload rejection/uncertainty does not consume or fence the previous
        // recipient card. Its existing callback still belongs to the user.
        let markup = native_lark::markup(&old).unwrap().0;
        let old = PreparedCard {
            card: old,
            version: 1,
            markup,
        };
        let callback = f.callback(&old, "old-card-still-usable", 0);
        f.store
            .consume_native_lark(&f.channel, &callback)
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn native_lark_image_replacement_unknown_patch_never_reuploads_or_replays() {
    let f = Fixture::new(false, false, LarkRegion::Lark).await;
    let (mut sender, api) = media::sender(&f);
    sender.relay = sender.relay.with_native_lark_media_card_replacements(true);
    let gate = RecordingGate::new(true);
    let ctx = SendContext {
        trace: &tracing::Span::none(),
        channel: &f.channel,
        first_send_done: false,
        gate: &gate,
    };
    let sent = sender.reply(&ctx, &f.event, &message(1)).await.0;
    let reference = sent
        .message_ref
        .unwrap()
        .open(&sender.keys)
        .unwrap()
        .payload;
    api.fault.store(5, Ordering::SeqCst);
    let result = sender.update(&ctx, &reference, &message(4)).await.0;
    assert_eq!(result.delivery.kind(), "unknown");
    assert!(result.message_ref.is_none());
    assert_eq!(writes(&api), (5, 1));
    api.fault.store(0, Ordering::SeqCst);
    let retry = sender.update(&ctx, &reference, &message(4)).await.0;
    assert_ne!(retry.delivery.kind(), "confirmed");
    assert!(retry.message_ref.is_none());
    assert_eq!(writes(&api), (5, 1));
}

#[tokio::test]
async fn native_lark_image_replacement_stops_on_source_or_consumer_revocation_between_uploads() {
    for consumer in [false, true] {
        let f = Fixture::new(true, true, LarkRegion::Lark).await;
        let (mut sender, api) = media::sender(&f);
        sender.relay = sender.relay.with_native_lark_media_card_replacements(true);
        let gate = RecordingGate::new(true);
        let ctx = SendContext {
            trace: &tracing::Span::none(),
            channel: &f.channel,
            first_send_done: false,
            gate: &gate,
        };
        let sent = sender.reply(&ctx, &f.event, &message(1)).await.0;
        let reference = sent
            .message_ref
            .unwrap()
            .open(&sender.keys)
            .unwrap()
            .payload;
        let (old, _) = f
            .store
            .native_lark_reference(&f.channel, &reference)
            .await
            .unwrap();
        let memory = f.server.memory.clone();
        let mut value = f.channel.definition.to_blob_json();
        if consumer {
            value["routing"]["consumers"][0]["provider"]["binding_id"] = "rebound".into();
        } else {
            value["sources"] = json!([]);
        }
        let record = f.channel.definition.record_id.to_string();
        *api.after_upload.lock().unwrap() = Some(Box::new(move || {
            memory
                .channels
                .lock()
                .unwrap()
                .get_mut(&record)
                .unwrap()
                .blob = cmaeg_core::seal::seal_blob(
                &crate::ingress::routing_plan::fixture::keys(),
                &serde_json::to_vec(&value).unwrap(),
            )
            .unwrap();
        }));
        let result = sender.update(&ctx, &reference, &message(4)).await.0;
        assert_ne!(result.delivery.kind(), "confirmed");
        assert!(result.message_ref.is_none());
        assert_eq!(writes(&api), (2, 0));
        assert_eq!(
            serde_json::to_value(f.store.load(&old.id).await.unwrap().0).unwrap(),
            serde_json::to_value(old).unwrap()
        );
    }
}
