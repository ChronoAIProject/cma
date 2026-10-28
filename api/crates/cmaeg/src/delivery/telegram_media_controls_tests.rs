use super::{
    gate::Ungated,
    intents::DeliveryOutcome,
    send::SendContext,
    telegram_media_controls_fixture::{message, Fixture},
};
use crate::{
    channels::definition::fixture::{replace_default_binding, replace_default_credential},
    ingress::{routing_plan::fixture, routing_store::test_server::Fault},
    interactions::{Destination, Error},
    telegram::media_fixture::{self, Answer},
};
use cmaeg_core::channel::Secret;
use serde_json::json;
use std::sync::Arc;

#[tokio::test]
async fn telegram_media_controls_default_off_refuses_without_records_or_platform_write() {
    let mut f = Fixture::new(false).await;
    f.sender.relay = f.sender.relay.with_telegram_media_controls(false);
    assert!(matches!(
        f.send(&message(false, 4)).await.delivery,
        DeliveryOutcome::Unsupported
    ));
    assert!(f.platform.calls().is_empty());
    assert!(f.records.memory.rows.lock().unwrap().is_empty());
}

#[tokio::test]
async fn telegram_media_controls_one_upload_contains_caption_and_keyboard_without_retaining_bytes()
{
    for named in [false, true] {
        for choice in [false, true] {
            let f = Fixture::new(named).await;
            let receipt = f.send(&message(choice, 60 * 1024)).await;
            assert!(
                matches!(receipt.delivery, DeliveryOutcome::Confirmed { .. }),
                "{:?}",
                receipt.delivery
            );
            let reference = receipt
                .message_ref
                .unwrap()
                .open(&fixture::keys())
                .unwrap()
                .payload;
            let (card, _) = f
                .store
                .load(reference.interaction_record_id.as_deref().unwrap())
                .await
                .unwrap();
            assert!(card.native_telegram_media.is_some());
            assert!(card.message.native_attachments.is_empty());
            assert!(serde_json::to_vec(&card).unwrap().len() < 8192);
            assert_eq!(reference.platform_message_id, "800");
            assert_eq!(reference.consumer, f.event.consumer);
            assert_eq!(card.expires_at, f.event.expires_at);
            assert_eq!(f.provider.seen().len(), 1);
            let calls = f.platform.calls();
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0].path, "/bot77:fixture/sendDocument");
            assert_eq!(media_fixture::field(&calls[0], "message_thread_id"), "7");
            assert_eq!(
                media_fixture::field(&calls[0], "caption"),
                if choice {
                    "Inspect this file\n\nPick one\n\nFile caption"
                } else {
                    "Inspect this file\n\nFile caption"
                }
            );
            let data = f.locator();
            assert!(data.len() <= 64);
            assert!(!data.contains("accept"));
        }
    }
}

#[tokio::test]
async fn telegram_media_controls_callback_retries_keep_original_owner_and_consume_once() {
    for named in [false, true] {
        let mut f = Fixture::new(named).await;
        let receipt = f.send(&message(true, 4)).await;
        assert!(matches!(
            receipt.delivery,
            DeliveryOutcome::Confirmed { .. }
        ));
        // Turning issuance off does not disable callbacks for already sent cards.
        f.sender.relay = f.sender.relay.with_telegram_media_controls(false);
        let data = f.locator();
        let mut click = f.activity.clone();
        click.event_id = "telegram:77:callback-1".parse().unwrap();
        let consume = || {
            f.store.consume_for_dispatch(
                &f.channel,
                &click,
                "telegram:77:-42",
                "query-1",
                &data,
                "800",
                None,
            )
        };
        let (first, second) = tokio::join!(consume(), consume());
        let first = first.unwrap();
        let second = second.unwrap();
        assert_eq!(first.0, second.0);
        assert_eq!(first.1, second.1);
        assert_eq!(first.0.values["selected"], json!(["accept"]));
        let owner = first.2.unwrap();
        assert_eq!(owner.consumer, f.event.consumer);
        assert_eq!(owner.conversation_id, f.event.conversation_id);
        assert_eq!(owner.partition_key, f.event.partition_key);
        assert!(matches!(
            f.store
                .consume_for_dispatch(
                    &f.channel,
                    &click,
                    "telegram:77:-42",
                    "query-2",
                    &data,
                    "800",
                    None
                )
                .await,
            Err(Error::Denied)
        ));
        assert_eq!(f.platform.calls().len(), 1);
    }
}

#[tokio::test]
async fn telegram_media_controls_callback_fences_actor_chat_topic_message_source_and_binding() {
    let f = Fixture::new(false).await;
    assert!(matches!(
        f.send(&message(false, 4)).await.delivery,
        DeliveryOutcome::Confirmed { .. }
    ));
    let data = f.locator();
    for changed in 0..7 {
        let mut activity = f.activity.clone();
        activity.event_id = "callback-test".parse().unwrap();
        let mut channel = f.channel.clone();
        let mut id = "800";
        match changed {
            0 => activity.actor.id = "8".into(),
            1 => activity.conversation.id = "-43".into(),
            2 => activity.conversation.thread_id = Some("8".into()),
            3 => id = "801",
            4 => activity.source = cmaeg_core::activity::SourceKind::Custom,
            5 => replace_default_binding(
                &mut Arc::make_mut(&mut channel).definition,
                "replacement".parse().unwrap(),
            ),
            6 => Arc::make_mut(&mut channel).definition.sources.clear(),
            _ => unreachable!(),
        }
        assert!(
            matches!(
                f.store
                    .consume_for_dispatch(
                        &channel,
                        &activity,
                        "telegram:77:-42",
                        "query",
                        &data,
                        id,
                        None
                    )
                    .await,
                Err(Error::Denied)
            ),
            "case {changed}"
        );
    }
    let mut rotated = f.channel.clone();
    Arc::make_mut(&mut rotated)
        .definition
        .secrets
        .telegram_bot_token = Some(Secret::new("77:rotated"));
    replace_default_credential(
        &mut Arc::make_mut(&mut rotated).definition,
        Some("provider-rotated"),
    );
    assert!(f
        .store
        .consume_for_dispatch(
            &rotated,
            &f.activity,
            "telegram:77:-42",
            "query",
            &data,
            "800",
            None
        )
        .await
        .is_ok());
}

#[tokio::test]
async fn telegram_media_controls_unconfirmed_or_wrong_topic_receipt_never_retries_or_binds() {
    for failure in 0..3 {
        let f = Fixture::new(false).await;
        let answer = if failure == 2 {
            Answer::json(
                reqwest::StatusCode::TOO_MANY_REQUESTS,
                json!({"ok":false,"error_code":429,"parameters":{"retry_after":5}}),
            )
        } else if failure == 1 {
            Answer::json(
                reqwest::StatusCode::OK,
                json!({"ok":true,"result":{"message_id":800,"chat":{"id":-42},"message_thread_id":8}}),
            )
        } else {
            let mut answer = Answer::ok();
            answer.broken = true;
            answer
        };
        f.platform.answer(answer);
        let receipt = f.send(&message(false, 4)).await;
        if failure == 2 {
            assert!(
                matches!(
                    receipt.delivery,
                    DeliveryOutcome::RateLimited { retry_after: 5 }
                ),
                "{:?}",
                receipt.delivery
            );
        } else {
            assert!(
                matches!(receipt.delivery, DeliveryOutcome::Unknown { .. }),
                "{:?}",
                receipt.delivery
            );
        }
        assert!(receipt.message_ref.is_none());
        assert_eq!(f.platform.calls().len(), 1);
        assert!(matches!(
            f.store
                .consume_for_dispatch(
                    &f.channel,
                    &f.activity,
                    "telegram:77:-42",
                    "query",
                    &f.locator(),
                    "800",
                    None
                )
                .await,
            Err(Error::Unavailable)
        ));
    }
}

#[tokio::test]
async fn telegram_media_controls_supports_each_single_upload_kind() {
    use cmaeg_core::message::NativeMediaKind;
    for (kind, method) in [
        (NativeMediaKind::Image, "sendPhoto"),
        (NativeMediaKind::File, "sendDocument"),
        (NativeMediaKind::Audio, "sendAudio"),
        (NativeMediaKind::Video, "sendVideo"),
    ] {
        let f = Fixture::new(false).await;
        let mut message = message(false, 4);
        message.native_attachments[0].kind = kind;
        let receipt = f.send(&message).await;
        assert!(
            matches!(receipt.delivery, DeliveryOutcome::Confirmed { .. }),
            "{:?}",
            receipt.delivery
        );
        let calls = f.platform.calls();
        assert_eq!(calls.len(), 1);
        assert!(calls[0].path.ends_with(method));
        assert!(!media_fixture::field(&calls[0], "reply_markup").is_empty());
    }
}

#[tokio::test]
async fn telegram_media_controls_binding_recovers_only_exact_committed_lost_ack() {
    for (committed, missing_read) in [(true, false), (false, false), (true, true)] {
        let f = Fixture::new(false).await;
        let memory = f.records.memory.clone();
        *f.platform.memory.on_send.lock().unwrap() = Some(Arc::new(move |request| {
            let markup: serde_json::Value =
                serde_json::from_str(&media_fixture::field(request, "reply_markup")).unwrap();
            let locator = markup["inline_keyboard"][0][0]["callback_data"]
                .as_str()
                .unwrap();
            let id = uuid::Uuid::parse_str(locator.split(':').nth(1).unwrap())
                .unwrap()
                .to_string();
            *memory.fault.lock().unwrap() = Some(Fault {
                id,
                store: committed,
                lose_read: missing_read,
                skip: 0,
            });
        }));
        let result = f.send(&message(false, 4)).await;
        if committed && !missing_read {
            assert!(
                matches!(result.delivery, DeliveryOutcome::Confirmed { .. }),
                "{:?}",
                result.delivery
            );
            assert!(result.message_ref.is_some());
        } else {
            assert!(matches!(result.delivery, DeliveryOutcome::Unknown { .. }));
            assert!(result.message_ref.is_none());
        }
        assert_eq!(f.platform.calls().len(), 1);
    }
}

#[tokio::test]
async fn telegram_media_controls_revalidates_destination_and_uses_current_same_bot_credentials() {
    for revoked in [false, true] {
        let f = Fixture::new(false).await;
        let memory = f.records.memory.clone();
        let id = f.channel.definition.record_id.to_string();
        {
            let mut rows = memory.channels.lock().unwrap();
            let row = rows.get_mut(&id).unwrap();
            let opened = cmaeg_core::seal::open_blob(&fixture::keys(), &row.blob).unwrap();
            let mut value: serde_json::Value = serde_json::from_slice(&opened.payload).unwrap();
            if revoked {
                value["sources"][0]["bot_id"] = "78".into();
            }
            value["secrets"]["telegram_bot_token"] =
                if revoked { "78:rotated" } else { "77:rotated" }.into();
            row.blob =
                cmaeg_core::seal::seal_blob(&fixture::keys(), &serde_json::to_vec(&value).unwrap())
                    .unwrap();
        }
        let receipt = f.send(&message(false, 4)).await;
        if revoked {
            assert!(matches!(receipt.delivery, DeliveryOutcome::Rejected { .. }));
            assert!(f.platform.calls().is_empty());
        } else {
            assert!(
                matches!(receipt.delivery, DeliveryOutcome::Confirmed { .. }),
                "{:?}",
                receipt.delivery
            );
            assert_eq!(f.platform.calls()[0].path, "/bot77:rotated/sendDocument");
        }
    }
}

#[tokio::test]
async fn telegram_media_controls_update_is_explicitly_refused_without_text_edit_or_card_fencing() {
    let f = Fixture::new(false).await;
    let reference = f
        .send(&message(false, 4))
        .await
        .message_ref
        .unwrap()
        .open(&fixture::keys())
        .unwrap()
        .payload;
    let trace = tracing::Span::none();
    let ctx = SendContext {
        trace: &trace,
        channel: &f.channel,
        first_send_done: false,
        gate: &Ungated,
    };
    let update = serde_json::from_value(json!({"text":"Updated caption"})).unwrap();
    let result = f.sender.native_update(&ctx, &reference, &update).await.0;
    assert!(matches!(result.delivery, DeliveryOutcome::Rejected { .. }));
    assert_eq!(f.platform.calls().len(), 1);
    let (card, _) = f
        .store
        .load(reference.interaction_record_id.as_deref().unwrap())
        .await
        .unwrap();
    assert!(!card.superseded);
}

#[tokio::test]
async fn telegram_media_controls_rejects_split_messages_oversize_caption_and_expired_card() {
    for invalid in 0..4 {
        let f = Fixture::new(false).await;
        let mut message = message(false, 4);
        match invalid {
            0 => message
                .native_attachments
                .push(message.native_attachments[0].clone()),
            1 => message.text = Some("😀".repeat(513)),
            2 => message.thread_reply = true,
            3 => message.blocks.push(
                serde_json::from_value(json!({"type":"card","title":"Unsupported presentation"}))
                    .unwrap(),
            ),
            _ => unreachable!(),
        }
        let receipt = f.send(&message).await;
        if invalid == 1 {
            assert!(
                matches!(receipt.delivery, DeliveryOutcome::Rejected { ref reason }
                if reason == "interactive_media_caption_too_long")
            );
        } else {
            assert!(matches!(receipt.delivery, DeliveryOutcome::Unsupported));
        }
        assert!(f.platform.calls().is_empty());
    }
    let f = Fixture::new(false).await;
    let mut card = f.prepared().await;
    let destination = Destination {
        outbound_id: "800".into(),
        platform_id: "800".into(),
    };
    f.store
        .bind_telegram_media(&mut card, destination)
        .await
        .unwrap();
    f.records.change(&card.card.id, |value| {
        value["expires_at"] = (time::OffsetDateTime::now_utc().unix_timestamp() - 1).into()
    });
    assert!(matches!(
        f.store.load(&card.card.id).await,
        Err(Error::Denied)
    ));
    assert!(f.platform.calls().is_empty());
}
