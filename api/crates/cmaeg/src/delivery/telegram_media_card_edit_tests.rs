use super::{
    gate::Ungated,
    intents::{DeliveryOutcome, IntentReceipt},
    send::SendContext,
    telegram_media_controls_fixture::{message, Fixture},
};
use crate::{
    ingress::{routing_plan::fixture, routing_store::test_server::Fault},
    interactions::{Error, InteractionStore},
    telegram::media_fixture::Answer,
};
use cmaeg_core::{
    channel::Secret,
    message::{NativeMediaKind, OutboundMessage},
    seal::MessageRefPayload,
};
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};

fn update_message(text: &str) -> OutboundMessage {
    let mut value = message(false, 4);
    value.native_attachments.clear();
    value.text = Some(text.into());
    value
}
async fn ready(named: bool, kind: NativeMediaKind) -> (Fixture, MessageRefPayload) {
    let mut f = Fixture::new(named).await;
    f.sender.relay = f.sender.relay.with_telegram_media_control_edits(true);
    let mut original = message(false, 4);
    original.native_attachments[0].kind = kind;
    let result = f.send(&original).await;
    assert!(matches!(result.delivery, DeliveryOutcome::Confirmed { .. }));
    let reference = result
        .message_ref
        .unwrap()
        .open(&fixture::keys())
        .unwrap()
        .payload;
    (f, reference)
}
async fn update(
    f: &Fixture,
    reference: &MessageRefPayload,
    message: &OutboundMessage,
) -> IntentReceipt {
    let trace = tracing::Span::none();
    let ctx = SendContext {
        trace: &trace,
        channel: &f.channel,
        first_send_done: false,
        gate: &Ungated,
    };
    f.sender.native_update(&ctx, reference, message).await.0
}
fn confirmed(receipt: IntentReceipt) -> MessageRefPayload {
    assert!(
        matches!(receipt.delivery, DeliveryOutcome::Confirmed { .. }),
        "{:?}",
        receipt.delivery
    );
    receipt
        .message_ref
        .unwrap()
        .open(&fixture::keys())
        .unwrap()
        .payload
}
fn latest_locator(f: &Fixture) -> String {
    let calls = f.platform.calls();
    let body: Value = serde_json::from_slice(&calls.last().unwrap().body).unwrap();
    body["reply_markup"]["inline_keyboard"][0][0]["callback_data"]
        .as_str()
        .unwrap()
        .into()
}

#[tokio::test]
async fn telegram_media_card_edits_old_cards_never_gain_permission_after_flag_enables() {
    let mut f = Fixture::new(false).await;
    let original = confirmed(f.send(&message(false, 4)).await);
    let before = f
        .store
        .load(original.interaction_record_id.as_deref().unwrap())
        .await
        .unwrap();
    let authority = serde_json::to_value(&before.0.native_telegram_media).unwrap();
    assert!(authority.get("snapshot").is_none());
    assert!(authority.get("edit").is_none());
    f.sender.relay = f.sender.relay.with_telegram_media_control_edits(true);
    assert!(matches!(
        update(&f, &original, &update_message("Later"))
            .await
            .delivery,
        DeliveryOutcome::Rejected { .. }
    ));
    let after = f
        .store
        .load(original.interaction_record_id.as_deref().unwrap())
        .await
        .unwrap();
    assert_eq!(before.1, after.1);
    assert_eq!(
        serde_json::to_value(before.0).unwrap(),
        serde_json::to_value(after.0).unwrap()
    );
    assert_eq!(f.platform.calls().len(), 1);
}

#[tokio::test]
async fn telegram_media_card_edits_invalid_replacements_refuse_before_mutation() {
    let (f, original) = ready(false, NativeMediaKind::File).await;
    let mut oversized = update_message(&"😀".repeat(513));
    oversized.blocks.clear();
    let mut threaded = update_message("Thread");
    threaded.thread_reply = true;
    let patch = serde_json::from_value(json!({"native_media_update":{"caption":"patch"}})).unwrap();
    for invalid in [oversized, threaded, message(false, 4), patch] {
        assert!(matches!(
            update(&f, &original, &invalid).await.delivery,
            DeliveryOutcome::Rejected { .. }
        ));
    }
    assert_eq!(f.platform.calls().len(), 1);
    confirmed(update(&f, &original, &update_message("Valid later")).await);
    assert_eq!(f.platform.calls().len(), 2);
}

#[tokio::test]
async fn telegram_media_card_edits_preserve_each_media_kind_and_original_expiry() {
    for kind in [
        NativeMediaKind::Image,
        NativeMediaKind::File,
        NativeMediaKind::Audio,
        NativeMediaKind::Video,
    ] {
        let (f, original) = ready(false, kind).await;
        let before = f
            .store
            .load(original.interaction_record_id.as_deref().unwrap())
            .await
            .unwrap()
            .0;
        let revised = confirmed(update(&f, &original, &update_message("Changed caption")).await);
        let after = f
            .store
            .load(revised.interaction_record_id.as_deref().unwrap())
            .await
            .unwrap()
            .0;
        assert_ne!(
            original.interaction_record_id,
            revised.interaction_record_id
        );
        assert_eq!(revised.platform_message_id, "800");
        assert_eq!(after.revision, 2);
        assert_eq!(after.expires_at, before.expires_at);
        let old = before.native_telegram_media.unwrap().snapshot.unwrap();
        let next = after.native_telegram_media.unwrap().snapshot.unwrap();
        assert!(old.same_files(&next));
        assert_eq!(next.caption, "Changed caption");
        let calls = f.platform.calls();
        assert_eq!(calls.len(), 2);
        assert!(calls[1].path.ends_with("/editMessageCaption"));
        let body: Value = serde_json::from_slice(&calls[1].body).unwrap();
        assert_eq!(body["chat_id"], "-42");
        assert_eq!(body["message_id"], "800");
        assert_eq!(body["caption"], "Changed caption");
        assert!(body.get("media").is_none());
    }
}

#[tokio::test]
async fn telegram_media_card_edits_support_markup_only_and_clearing_caption_and_controls() {
    let (f, original) = ready(false, NativeMediaKind::File).await;
    let mut markup = update_message("ignored");
    markup.text = None;
    let revised = confirmed(update(&f, &original, &markup).await);
    let record = f
        .store
        .load(revised.interaction_record_id.as_deref().unwrap())
        .await
        .unwrap()
        .0;
    assert_eq!(
        record
            .native_telegram_media
            .unwrap()
            .snapshot
            .unwrap()
            .caption,
        "Inspect this file\n\nFile caption"
    );
    let empty = serde_json::from_value(json!({"text":""})).unwrap();
    let cleared = confirmed(update(&f, &revised, &empty).await);
    let record = f
        .store
        .load(cleared.interaction_record_id.as_deref().unwrap())
        .await
        .unwrap()
        .0;
    assert_eq!(
        record
            .native_telegram_media
            .unwrap()
            .snapshot
            .unwrap()
            .caption,
        ""
    );
    let calls = f.platform.calls();
    let body: Value = serde_json::from_slice(&calls[2].body).unwrap();
    assert_eq!(body["reply_markup"], json!({"inline_keyboard":[]}));
    assert!(calls.iter().all(|c| !c.path.ends_with("/editMessageText")));
}

#[tokio::test]
async fn telegram_media_card_edits_preserve_caption_presentation_and_automatic_entities() {
    let mut f = Fixture::new(false).await;
    f.sender.relay = f.sender.relay.with_telegram_media_control_edits(true);
    let weak = Arc::downgrade(&f.platform.memory);
    *f.platform.memory.on_send.lock().unwrap() = Some(Arc::new(move |request| {
        let memory = weak.upgrade().unwrap();
        let markup: Value = serde_json::from_str(&crate::telegram::media_fixture::field(
            request,
            "reply_markup",
        ))
        .unwrap();
        let caption = crate::telegram::media_fixture::field(request, "caption");
        let value = json!({"message_id":800,"from":{"id":77,"is_bot":true},"chat":{"id":-42},"message_thread_id":7,
            "photo":[{"file_id":"locator","file_unique_id":"image","width":2,"height":2}],
            "caption":caption,"reply_markup":markup,"show_caption_above_media":true,"has_media_spoiler":true,
            "caption_entities":[{"type":"url","offset":0,"length":19}]});
        *memory.last_media.lock().unwrap() = Some(value.clone());
        memory.answers.lock().unwrap().push_back(Answer::json(
            reqwest::StatusCode::OK,
            json!({"ok":true,"result":value}),
        ));
    }));
    let mut initial = message(false, 4);
    initial.text = Some("https://example.org".into());
    initial.native_attachments[0].kind = NativeMediaKind::Image;
    let original = confirmed(f.send(&initial).await);
    *f.platform.memory.on_send.lock().unwrap() = None;
    let mut markup = update_message("ignored");
    markup.text = None;
    let revised = confirmed(update(&f, &original, &markup).await);
    let body: Value = serde_json::from_slice(&f.platform.calls()[1].body).unwrap();
    assert_eq!(
        body["caption_entities"],
        json!([{"type":"url","offset":0,"length":19}])
    );
    assert_eq!(body["show_caption_above_media"], true);
    let latest = confirmed(update(&f, &revised, &update_message("New caption")).await);
    let card = f
        .store
        .load(latest.interaction_record_id.as_deref().unwrap())
        .await
        .unwrap()
        .0;
    let snapshot = card.native_telegram_media.unwrap().snapshot.unwrap();
    assert!(matches!(snapshot.presentation,
        Some(crate::native_media::model::Presentation::Telegram { entities, caption_above: true, spoiler: true }) if entities.is_empty()));
}

#[tokio::test]
async fn telegram_media_card_edits_keep_callback_owner_and_exact_consumed_replays() {
    for named in [false, true] {
        let (f, original) = ready(named, NativeMediaKind::File).await;
        let old_locator = f.locator();
        let mut click = f.activity.clone();
        click.event_id = "telegram:77:original-click".parse().unwrap();
        let first = f
            .store
            .consume_for_dispatch(
                &f.channel,
                &click,
                "telegram:77:-42",
                "old-query",
                &old_locator,
                "800",
                None,
            )
            .await
            .unwrap();
        let revised = confirmed(update(&f, &original, &update_message("Updated")).await);
        let replay = f
            .store
            .consume_for_dispatch(
                &f.channel,
                &click,
                "telegram:77:-42",
                "old-query",
                &old_locator,
                "800",
                None,
            )
            .await
            .unwrap();
        assert_eq!(first.0, replay.0);
        assert_eq!(first.1, replay.1);
        assert!(matches!(
            f.store
                .consume_for_dispatch(
                    &f.channel,
                    &click,
                    "telegram:77:-42",
                    "foreign-query",
                    &old_locator,
                    "800",
                    None
                )
                .await,
            Err(Error::Denied)
        ));
        click.event_id = "telegram:77:updated-click".parse().unwrap();
        let next = f
            .store
            .consume_for_dispatch(
                &f.channel,
                &click,
                "telegram:77:-42",
                "new-query",
                &latest_locator(&f),
                "800",
                None,
            )
            .await
            .unwrap();
        assert_eq!(next.2.unwrap().consumer, original.consumer);
        assert_eq!(
            next.0.values["interaction_record_id"],
            revised.interaction_record_id.unwrap()
        );
    }
}

#[tokio::test]
async fn telegram_media_card_edits_exact_retry_survives_restart_and_competing_requests() {
    let (mut f, original) = ready(false, NativeMediaKind::File).await;
    let requested = update_message("One edit");
    let (first, second) = tokio::join!(
        update(&f, &original, &requested),
        update(&f, &original, &requested)
    );
    assert!(
        matches!(first.delivery, DeliveryOutcome::Confirmed { .. })
            || matches!(second.delivery, DeliveryOutcome::Confirmed { .. })
    );
    assert_eq!(f.platform.calls().len(), 2);
    let store = Arc::new(InteractionStore::new(
        f.records.records.clone(),
        Arc::new(fixture::keys()),
        f.provider.client(true, Duration::from_secs(2)),
    ));
    f.sender.relay = f
        .sender
        .relay
        .with_interactions(store.clone())
        .with_telegram_media_control_edits(false);
    f.store = store;
    let replay = confirmed(update(&f, &original, &requested).await);
    assert_eq!(replay.platform_message_id, "800");
    assert_eq!(f.platform.calls().len(), 2);
    assert!(matches!(
        update(&f, &original, &update_message("Different edit"))
            .await
            .delivery,
        DeliveryOutcome::Rejected { .. }
    ));
    assert_eq!(f.platform.calls().len(), 2);
}

#[tokio::test]
async fn telegram_media_card_edits_lost_platform_response_never_retries_or_revives_buttons() {
    let (f, original) = ready(false, NativeMediaKind::File).await;
    let locator = f.locator();
    let mut answer = Answer::ok();
    answer.broken = true;
    f.platform.answer(answer);
    for _ in 0..2 {
        assert!(matches!(
            update(&f, &original, &update_message("Maybe edited"))
                .await
                .delivery,
            DeliveryOutcome::Unknown { .. }
        ));
    }
    assert_eq!(f.platform.calls().len(), 2);
    assert!(matches!(
        f.store
            .consume_for_dispatch(
                &f.channel,
                &f.activity,
                "telegram:77:-42",
                "click",
                &locator,
                "800",
                None
            )
            .await,
        Err(Error::Denied)
    ));
}

#[tokio::test]
async fn telegram_media_card_edits_refuse_changed_files_topic_or_keyboard_receipts() {
    for invalid in 0..3 {
        let (f, original) = ready(false, NativeMediaKind::File).await;
        let weak = Arc::downgrade(&f.platform.memory);
        *f.platform.memory.on_send.lock().unwrap() = Some(Arc::new(move |request| {
            let memory = weak.upgrade().unwrap();
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            let mut value = memory.last_media.lock().unwrap().clone().unwrap();
            value["caption"] = body["caption"].clone();
            value["reply_markup"] = body["reply_markup"].clone();
            match invalid {
                0 => value["document"]["file_unique_id"] = "changed_file".into(),
                1 => value["message_thread_id"] = 8.into(),
                2 => value["reply_markup"] = json!({"inline_keyboard":[]}),
                _ => unreachable!(),
            }
            memory.answers.lock().unwrap().push_back(Answer::json(
                reqwest::StatusCode::OK,
                json!({"ok":true,"result":value}),
            ));
        }));
        assert!(matches!(
            update(&f, &original, &update_message("Changed"))
                .await
                .delivery,
            DeliveryOutcome::Unknown { .. }
        ));
        assert!(matches!(
            update(&f, &original, &update_message("Changed"))
                .await
                .delivery,
            DeliveryOutcome::Unknown { .. }
        ));
        assert_eq!(f.platform.calls().len(), 2);
    }
}

#[tokio::test]
async fn telegram_media_card_edits_lost_final_receipt_recovers_bound_successor_and_unlocks() {
    for committed in [false, true] {
        let (f, original) = ready(false, NativeMediaKind::File).await;
        let memory = f.records.memory.clone();
        let id = original.interaction_record_id.clone().unwrap();
        *f.platform.memory.on_send.lock().unwrap() = Some(Arc::new(move |_| {
            *memory.fault.lock().unwrap() = Some(Fault {
                id: id.clone(),
                store: committed,
                lose_read: false,
                skip: 0,
            });
        }));
        let first = update(&f, &original, &update_message("Recovered")).await;
        if committed {
            assert!(matches!(first.delivery, DeliveryOutcome::Confirmed { .. }));
        } else {
            assert!(matches!(first.delivery, DeliveryOutcome::Unknown { .. }));
        }
        *f.platform.memory.on_send.lock().unwrap() = None;
        let revised = confirmed(update(&f, &original, &update_message("Recovered")).await);
        assert_eq!(f.platform.calls().len(), 2);
        confirmed(update(&f, &revised, &update_message("Next edit")).await);
        assert_eq!(f.platform.calls().len(), 3);
    }
}

#[tokio::test]
async fn telegram_media_card_edits_lost_binding_response_and_read_recovers_without_resend() {
    let (f, original) = ready(false, NativeMediaKind::File).await;
    let memory = f.records.memory.clone();
    *f.platform.memory.on_send.lock().unwrap() = Some(Arc::new(move |request| {
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        let locator = body["reply_markup"]["inline_keyboard"][0][0]["callback_data"]
            .as_str()
            .unwrap();
        let id = uuid::Uuid::parse_str(locator.split(':').nth(1).unwrap())
            .unwrap()
            .to_string();
        *memory.fault.lock().unwrap() = Some(Fault {
            id,
            store: true,
            lose_read: true,
            skip: 0,
        });
    }));
    let request = update_message("Bound but read lost");
    assert!(matches!(
        update(&f, &original, &request).await.delivery,
        DeliveryOutcome::Unknown { .. }
    ));
    *f.platform.memory.on_send.lock().unwrap() = None;
    let successor = confirmed(update(&f, &original, &request).await);
    assert_eq!(f.platform.calls().len(), 2);
    confirmed(update(&f, &successor, &update_message("Continue")).await);
    assert_eq!(f.platform.calls().len(), 3);
}

#[tokio::test]
async fn telegram_media_card_edit_replay_cannot_release_a_later_mutation_lock() {
    let (f, original) = ready(false, NativeMediaKind::File).await;
    let requested = update_message("First");
    let revised = confirmed(update(&f, &original, &requested).await);
    let serial = f.store.telegram_media_mutations();
    let destination = crate::tools::message_effects::mutation::destination("77", "-42", "800");
    let held = serial
        .acquire(
            destination,
            &f.channel.definition.owner.subject,
            f.event.expires_at,
        )
        .await
        .unwrap()
        .unwrap();
    confirmed(update(&f, &original, &requested).await);
    assert!(matches!(
        update(&f, &revised, &update_message("Second"))
            .await
            .delivery,
        DeliveryOutcome::Unknown { .. }
    ));
    assert_eq!(f.platform.calls().len(), 2);
    serial.release(held).await.unwrap();
    confirmed(update(&f, &revised, &update_message("Second")).await);
    assert_eq!(f.platform.calls().len(), 3);
}

#[tokio::test]
async fn telegram_media_card_edits_keep_same_bot_rotations_but_refuse_revoked_sources() {
    for revoked in [false, true] {
        let (mut f, original) = ready(false, NativeMediaKind::File).await;
        let channel = Arc::make_mut(&mut f.channel);
        channel.definition.secrets.telegram_bot_token = Some(Secret::new("77:rotated"));
        if revoked {
            channel.definition.sources.clear();
            channel.definition.reply.sinks.clear();
        }
        f.save_channel();
        let result = update(&f, &original, &update_message("Updated")).await;
        if revoked {
            assert!(
                matches!(result.delivery, DeliveryOutcome::Rejected { .. }),
                "{:?}",
                result.delivery
            );
            assert_eq!(f.platform.calls().len(), 1);
        } else {
            confirmed(result);
            assert_eq!(
                f.platform.calls()[1].path,
                "/bot77:rotated/editMessageCaption"
            );
        }
    }
}
