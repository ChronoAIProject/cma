use super::{native_lark_media_fixture::*, *};
use crate::interactions::{
    native_lark::{self, fixture::Fixture},
    PreparedCard,
};
use cmaeg_core::{channel::LarkRegion, message::NativeMediaSource};
use reqwest::Method;
use serde_json::Value;
use std::sync::atomic::Ordering;

#[tokio::test]
async fn native_lark_image_and_controls_share_one_exact_message_for_each_region_consumer_thread() {
    for region in [LarkRegion::Lark, LarkRegion::Feishu] {
        for named in [false, true] {
            for thread in [false, true] {
                for choice in [false, true] {
                    let f = Fixture::new(named, thread, region).await;
                    let (mut sender, api) = sender(&f);
                    let mut message =
                        message(choice, if choice { "image/jpeg" } else { "image/png" });
                    message.thread_reply = thread;
                    let gate = RecordingGate::new(true);
                    let ctx = SendContext {
                        trace: &tracing::Span::none(),
                        channel: &f.channel,
                        first_send_done: false,
                        gate: &gate,
                    };
                    let sent = sender.reply(&ctx, &f.event, &message).await.0;
                    assert_eq!(sent.delivery.kind(), "confirmed");
                    assert_eq!(
                        gate.calls()
                            .iter()
                            .filter(|call| **call == "attempt")
                            .count(),
                        2
                    );
                    let reference = sent
                        .message_ref
                        .unwrap()
                        .open(&sender.keys)
                        .unwrap()
                        .payload;
                    let (card, version) = f
                        .store
                        .native_lark_reference(&f.channel, &reference)
                        .await
                        .unwrap();
                    assert!(card.message.native_attachments.is_empty());
                    let stored = serde_json::to_string(&card).unwrap();
                    assert!(!stored.contains("base64"));
                    assert!(stored.contains("img_confirmed_card"));
                    assert!(stored.len() < 16 * 1024);
                    let markup = native_lark::markup(&card).unwrap().0;
                    assert_eq!(
                        markup["body"]["elements"][0],
                        json!({"tag":"img","img_key":"img_confirmed_card","alt":{"tag":"plain_text","content":""},"scale_type":"fit_horizontal","preview":true})
                    );
                    {
                        let calls = api.calls.lock().unwrap();
                        let writes: Vec<_> = calls
                            .iter()
                            .filter(|(m, u, _, _)| {
                                *m == Method::POST && !u.path().contains("tenant_access_token")
                            })
                            .collect();
                        assert_eq!(writes.len(), 2);
                        assert!(writes[0].1.path().ends_with("/images"));
                        assert!(writes[0].2["content-type"]
                            .to_str()
                            .unwrap()
                            .starts_with("multipart/form-data; boundary="));
                        let upload = String::from_utf8_lossy(&writes[0].3);
                        assert!(upload.contains("name=\"image_type\"\r\n\r\nmessage"));
                        assert!(upload.contains("name=\"image\""));
                        assert_eq!(sent_card(&calls), markup);
                        let host = if region == LarkRegion::Lark {
                            "open.larksuite.com"
                        } else {
                            "open.feishu.cn"
                        };
                        assert!(calls.iter().all(|(_, u, _, _)| u.host_str() == Some(host)));
                        let body: Value = serde_json::from_slice(&writes[1].3).unwrap();
                        if thread {
                            assert_eq!(body["reply_in_thread"], true);
                        }
                    }
                    // Stopping issuance must preserve edits and the exact confirmed image.
                    sender.relay = sender
                        .relay
                        .with_native_lark_media_controls(false)
                        .with_native_lark_controls(false, false);
                    message.native_attachments.clear();
                    message.text = Some("Updated caption text".into());
                    let updated = sender.update(&ctx, &reference, &message).await.0;
                    assert_eq!(updated.delivery.kind(), "confirmed");
                    let updated_ref = updated
                        .message_ref
                        .unwrap()
                        .open(&sender.keys)
                        .unwrap()
                        .payload;
                    let (new, new_version) = f
                        .store
                        .native_lark_reference(&f.channel, &updated_ref)
                        .await
                        .unwrap();
                    assert_eq!(new.expires_at, card.expires_at);
                    assert_eq!(new.consumer, card.consumer);
                    assert_eq!(
                        updated_ref.platform_message_id,
                        reference.platform_message_id
                    );
                    let next = PreparedCard {
                        markup: native_lark::markup(&new).unwrap().0,
                        card: new,
                        version: new_version,
                    };
                    assert_eq!(
                        next.markup["body"]["elements"][0],
                        markup["body"]["elements"][0]
                    );
                    let click = f.callback(&next, "image_clicked", 0);
                    let id = f
                        .store
                        .consume_native_lark(&f.channel, &click)
                        .await
                        .unwrap();
                    assert_eq!(
                        f.store
                            .consume_native_lark(&f.channel, &click)
                            .await
                            .unwrap(),
                        id
                    );
                    let pending = f.store.native_lark_pending(&id, &f.channel).await.unwrap();
                    assert_eq!(pending.event.consumer, f.event.consumer);
                    assert_eq!(pending.event.conversation_id, f.event.conversation_id);
                    assert_eq!(pending.activity.conversation.thread_id.is_some(), thread);
                    assert!(f
                        .store
                        .native_lark_recovery_page(None)
                        .await
                        .unwrap()
                        .cards
                        .iter()
                        .any(|(x, _)| x == &id));
                    native_lark::current_dispatch(
                        &f.server.records,
                        &sender.keys,
                        &f.channel,
                        &pending.activity,
                        &pending.event,
                    )
                    .await
                    .unwrap();
                    f.store.native_lark_admitted(&id).await.unwrap();
                    assert!(!f
                        .store
                        .native_lark_recovery_page(None)
                        .await
                        .unwrap()
                        .cards
                        .iter()
                        .any(|(x, _)| x == &id));
                    let old = PreparedCard {
                        card,
                        version,
                        markup,
                    };
                    assert!(f
                        .store
                        .consume_native_lark(&f.channel, &f.callback(&old, "old_revision", 0))
                        .await
                        .is_err());
                }
            }
        }
    }
}

#[tokio::test]
async fn native_lark_combination_refuses_disabled_gates_or_unsupported_shapes_before_any_platform_call(
) {
    for case in 0..9 {
        let f = Fixture::new(false, false, LarkRegion::Lark).await;
        let (mut sender, api) = sender(&f);
        let mut request = message(false, "image/png");
        match case {
            0 => sender.relay = sender.relay.with_native_lark_media_controls(false),
            1 => sender.relay = sender.relay.with_native_lark_outbound_media(false),
            2 => sender.relay = sender.relay.with_native_lark_controls(false, false),
            3 => request
                .native_attachments
                .push(request.native_attachments[0].clone()),
            4 => request = message(false, "text/plain"),
            5 => request.native_attachments[0].caption = Some("caption".into()),
            6 => {
                let image = request.native_attachments.clone();
                request = crate::interactions::native_lark::fixture::outbound(true);
                request.native_attachments = image;
            }
            7 => {
                request.native_attachments[0].source = NativeMediaSource::Base64 {
                    data: STANDARD.encode(b"invalid image"),
                }
            }
            8 => request.thread_reply = true,
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
async fn native_lark_uncertain_upload_send_or_receipt_never_issues_a_usable_card_reference() {
    for fault in 1..=4 {
        let f = Fixture::new(false, false, LarkRegion::Feishu).await;
        let (sender, api) = sender(&f);
        api.fault.store(fault, Ordering::SeqCst);
        let gate = RecordingGate::new(true);
        let ctx = SendContext {
            trace: &tracing::Span::none(),
            channel: &f.channel,
            first_send_done: false,
            gate: &gate,
        };
        let sent = sender
            .reply(&ctx, &f.event, &message(false, "image/png"))
            .await
            .0;
        assert!(sent.message_ref.is_none());
        assert_ne!(sent.delivery.kind(), "confirmed", "fault {fault}");
        let calls = api.calls.lock().unwrap();
        assert_eq!(
            calls
                .iter()
                .filter(|(m, u, _, _)| *m == Method::POST
                    && (u.path().ends_with("/messages") || u.path().ends_with("/reply")))
                .count(),
            usize::from(fault >= 3)
        );
    }
}

#[tokio::test]
async fn native_lark_revocation_between_upload_and_card_send_never_degrades_or_sends() {
    for named in [false, true] {
        for case in 0..3 {
            let f = Fixture::new(named, true, LarkRegion::Lark).await;
            let (sender, api) = sender(&f);
            let memory = f.server.memory.clone();
            let mut definition = f.channel.definition.clone();
            match case {
                0 => definition.sources.clear(),
                1 => {
                    definition.secrets.native_lark.as_mut().unwrap().app_secret =
                        Secret::new("rotated-fixture-secret")
                }
                2 => {
                    if named {
                        definition.routing.as_mut().unwrap().consumers[0]
                            .provider
                            .binding_id = "changed".parse().unwrap();
                    } else {
                        crate::channels::definition::fixture::replace_default_binding(
                            &mut definition,
                            "changed".parse().unwrap(),
                        );
                    }
                }
                _ => unreachable!(),
            }
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
            let gate = RecordingGate::new(true);
            let ctx = SendContext {
                trace: &tracing::Span::none(),
                channel: &f.channel,
                first_send_done: false,
                gate: &gate,
            };
            let sent = sender
                .reply(&ctx, &f.event, &message(false, "image/png"))
                .await
                .0;
            assert!(sent.message_ref.is_none());
            assert_ne!(sent.delivery.kind(), "confirmed");
            assert_eq!(
                api.calls
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|(m, u, _, _)| *m == Method::POST
                        && !u.path().contains("tenant_access_token"))
                    .count(),
                1
            );
        }
    }
}

#[tokio::test]
async fn native_lark_media_cards_preserve_callback_actor_and_current_provider_custody() {
    for named in [false, true] {
        let mut f = Fixture::new(named, true, LarkRegion::Feishu).await;
        let (sender, _) = sender(&f);
        let gate = RecordingGate::new(true);
        let ctx = SendContext {
            trace: &tracing::Span::none(),
            channel: &f.channel,
            first_send_done: false,
            gate: &gate,
        };
        let sent = sender
            .reply(&ctx, &f.event, &message(false, "image/png"))
            .await
            .0;
        let reference = sent
            .message_ref
            .unwrap()
            .open(&sender.keys)
            .unwrap()
            .payload;
        let (card, version) = f
            .store
            .native_lark_reference(&f.channel, &reference)
            .await
            .unwrap();
        let prepared = PreparedCard {
            markup: native_lark::markup(&card).unwrap().0,
            card,
            version,
        };
        let callback = f.callback(&prepared, "media_custody_click", 0);
        for case in 0..5 {
            let mut changed = callback.clone();
            match case {
                0 => changed.actor = "ou_different_actor".into(),
                1 => changed.chat = "oc_different_chat".into(),
                2 => changed.identity.app_id = "cli_different_app".into(),
                3 => changed.message = "om_different_message".into(),
                4 => changed.locator = changed.locator.replace("cnl1:", "cl1:"),
                _ => unreachable!(),
            }
            assert!(f
                .store
                .consume_native_lark(&f.channel, &changed)
                .await
                .is_err());
        }
        let id = f
            .store
            .consume_native_lark(&f.channel, &callback)
            .await
            .unwrap();
        let pending = f.store.native_lark_pending(&id, &f.channel).await.unwrap();
        let definition = &mut Arc::make_mut(&mut f.channel).definition;
        if named {
            definition.routing.as_mut().unwrap().consumers[0]
                .provider
                .binding_id = "different_provider".parse().unwrap();
        } else {
            crate::channels::definition::fixture::replace_default_binding(
                definition,
                "different_provider".parse().unwrap(),
            );
        }
        f.save_channel();
        assert!(f.store.native_lark_pending(&id, &f.channel).await.is_err());
        assert!(native_lark::current_dispatch(
            &f.server.records,
            &sender.keys,
            &f.channel,
            &pending.activity,
            &pending.event
        )
        .await
        .is_err());
    }
}

#[tokio::test]
async fn native_lark_failed_image_card_patch_stays_fenced_without_reupload_or_retry() {
    let f = Fixture::new(false, false, LarkRegion::Lark).await;
    let (sender, api) = sender(&f);
    let gate = RecordingGate::new(true);
    let ctx = SendContext {
        trace: &tracing::Span::none(),
        channel: &f.channel,
        first_send_done: false,
        gate: &gate,
    };
    let sent = sender
        .reply(&ctx, &f.event, &message(false, "image/png"))
        .await
        .0;
    let reference = sent
        .message_ref
        .unwrap()
        .open(&sender.keys)
        .unwrap()
        .payload;
    api.fault.store(5, Ordering::SeqCst);
    let edited = sender
        .update(
            &ctx,
            &reference,
            &crate::interactions::native_lark::fixture::outbound(false),
        )
        .await
        .0;
    assert_eq!(edited.delivery.kind(), "unknown");
    assert!(edited.message_ref.is_none());
    let retry = sender
        .update(
            &ctx,
            &reference,
            &crate::interactions::native_lark::fixture::outbound(false),
        )
        .await
        .0;
    assert!(retry.message_ref.is_none());
    let calls = api.calls.lock().unwrap();
    assert_eq!(
        calls
            .iter()
            .filter(|(m, u, _, _)| *m == Method::POST && u.path().ends_with("/images"))
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
}
