use super::*;
use crate::telegram::media_fixture::field;
use cmaeg_core::message::NativeMediaKind;
use serde_json::Value;

fn album(kinds: &[NativeMediaKind]) -> OutboundMessage {
    let mut result = message();
    result.native_attachments = kinds
        .iter()
        .enumerate()
        .map(|(i, kind)| {
            let mut item = result.native_attachments[0].clone();
            item.kind = *kind;
            item.filename = Some(format!("part-{i}.bin"));
            item.caption = Some(format!("Caption {i}"));
            item
        })
        .collect();
    result
}

fn result(message: &OutboundMessage, thread: Option<i64>) -> Value {
    Value::Array(
        message
            .native_attachments
            .iter()
            .enumerate()
            .map(|(i, item)| {
                let kind = match item.kind {
                    NativeMediaKind::Image => "photo",
                    NativeMediaKind::Video => "video",
                    NativeMediaKind::File => "document",
                    NativeMediaKind::Audio => "audio",
                    NativeMediaKind::Sticker => unreachable!(),
                };
                let file = json!({"file_id":format!("file-{i}")});
                let mut value = json!({
                    "message_id":800+i, "media_group_id":"group-1", "chat":{"id":-42},
                    "from":{"id":77,"is_bot":true}, "caption":item.caption,
                    kind:if kind == "photo" {json!([file])} else {file},
                });
                if let Some(thread) = thread {
                    value["message_thread_id"] = thread.into();
                }
                value
            })
            .collect(),
    )
}

fn answer(value: Value) -> Answer {
    Answer::json(reqwest::StatusCode::OK, json!({"ok":true,"result":value}))
}

#[tokio::test]
async fn native_telegram_album_preserves_order_captions_and_topic_with_one_authorized_write() {
    use NativeMediaKind::{Audio, File, Image, Video};
    for (kinds, thread) in [
        (vec![Image, Image], Some(7)),
        (vec![Image, Video, Image, Video], None),
        (vec![Audio, Audio, Audio], Some(7)),
        (vec![File, File], None),
    ] {
        let f = Fixture::new().await;
        let (mut sender, channel, mut event) = setup(&f);
        sender.relay = sender.relay.with_native_telegram_albums(true);
        if let ReplyAddress::Telegram { thread_id, .. } = &mut event.reply {
            *thread_id = thread.map(|id| id.to_string());
        }
        let message = album(&kinds);
        f.answer(answer(result(&message, thread)));
        let gate = RecordingGate::new(true);
        let receipt = send(&sender, &channel, &event, &message, &gate).await;
        assert!(
            matches!(receipt.delivery, DeliveryOutcome::Confirmed { platform_message_id: Some(ref id) } if id == "800")
        );
        assert!(receipt.message_ref.is_none());
        assert_eq!(
            gate.calls(),
            ["enter_route", "attempt", "commit", "progress"]
        );
        let calls = f.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].path, "/bot77:fixture/sendMediaGroup");
        assert_eq!(field(&calls[0], "chat_id"), "-42");
        assert_eq!(
            field(&calls[0], "message_thread_id"),
            thread.map(|id| id.to_string()).unwrap_or_default()
        );
        let media: Vec<Value> = serde_json::from_str(&field(&calls[0], "media")).unwrap();
        assert_eq!(media.len(), kinds.len());
        for (i, item) in media.iter().enumerate() {
            assert_eq!(item["media"], format!("attach://attachment_{i}"));
            assert_eq!(item["caption"], format!("Caption {i}"));
            assert!(String::from_utf8_lossy(&calls[0].body)
                .contains(&format!("filename=\"part-{i}.bin\"")));
        }
        assert!(field(&calls[0], "reply_markup").is_empty());
        assert!(!String::from_utf8_lossy(&calls[0].body).contains("77:fixture"));
    }
}

#[tokio::test]
async fn native_telegram_channel_album_accepts_optional_sender_and_fences_conflicting_identity() {
    for case in [
        "channel_sender",
        "absent_sender",
        "wrong_sender_chat",
        "wrong_sender_type",
        "wrong_destination",
        "non_channel",
        "wrong_bot",
        "human",
    ] {
        let f = Fixture::new().await;
        let (mut sender, channel, mut event) = setup(&f);
        sender.relay = sender.relay.with_native_telegram_albums(true);
        if let ReplyAddress::Telegram { thread_id, .. } = &mut event.reply {
            *thread_id = None;
        }
        let message = album(&[NativeMediaKind::Image; 2]);
        let mut value = result(&message, None);
        for member in value.as_array_mut().unwrap() {
            member.as_object_mut().unwrap().remove("from");
            member["chat"]["type"] = "channel".into();
            member["sender_chat"] = json!({"id":-42,"type":"channel"});
        }
        match case {
            "channel_sender" => {}
            "absent_sender" => {
                for member in value.as_array_mut().unwrap() {
                    member.as_object_mut().unwrap().remove("sender_chat");
                }
            }
            "wrong_sender_chat" => value[1]["sender_chat"]["id"] = (-43).into(),
            "wrong_sender_type" => value[1]["sender_chat"]["type"] = "supergroup".into(),
            "wrong_destination" => value[1]["chat"]["id"] = (-43).into(),
            "non_channel" => value[1]["chat"]["type"] = "supergroup".into(),
            "wrong_bot" => value[1]["from"] = json!({"id":78,"is_bot":true}),
            "human" => value[1]["from"] = json!({"id":77,"is_bot":false}),
            _ => unreachable!(),
        }
        f.answer(answer(value));
        let receipt = send(
            &sender,
            &channel,
            &event,
            &message,
            &RecordingGate::new(true),
        )
        .await;
        if matches!(case, "channel_sender" | "absent_sender") {
            assert!(
                matches!(receipt.delivery, DeliveryOutcome::Confirmed { platform_message_id: Some(ref id) } if id == "800"),
                "{case}"
            );
        } else {
            assert!(
                matches!(receipt.delivery, DeliveryOutcome::Unknown { .. }),
                "{case}"
            );
        }
        assert!(receipt.message_ref.is_none(), "{case}");
        assert_eq!(f.calls().len(), 1, "{case}");
        assert!(f.calls()[0].path.ends_with("/sendMediaGroup"), "{case}");
    }
}

#[tokio::test]
async fn native_telegram_album_gate_off_and_incompatible_kinds_keep_separate_uploads() {
    use NativeMediaKind::{Audio, File, Image};
    for (enabled, kinds) in [
        (false, vec![Image, Image]),
        (true, vec![Image, File]),
        (true, vec![Audio, Image]),
    ] {
        let f = Fixture::new().await;
        let (mut sender, channel, event) = setup(&f);
        sender.relay = sender.relay.with_native_telegram_albums(enabled);
        let receipt = send(
            &sender,
            &channel,
            &event,
            &album(&kinds),
            &RecordingGate::new(true),
        )
        .await;
        assert!(matches!(
            receipt.delivery,
            DeliveryOutcome::Confirmed { .. }
        ));
        assert!(receipt.message_ref.is_none());
        assert_eq!(f.calls().len(), 2);
        assert!(f
            .calls()
            .iter()
            .all(|c| !c.path.ends_with("sendMediaGroup")));
    }
}

#[tokio::test]
async fn native_telegram_album_all_members_must_match_the_original_destination_and_group() {
    use NativeMediaKind::Image;
    for case in [
        "count",
        "duplicate",
        "chat",
        "topic",
        "bot",
        "human",
        "group",
        "missing_group",
        "media",
        "caption",
        "id",
        "object",
    ] {
        let f = Fixture::new().await;
        let (mut sender, channel, event) = setup(&f);
        sender.relay = sender.relay.with_native_telegram_albums(true);
        let message = album(&[Image, Image]);
        let mut value = result(&message, Some(7));
        match case {
            "count" => {
                value.as_array_mut().unwrap().pop();
            }
            "duplicate" => value[1]["message_id"] = 800.into(),
            "chat" => value[1]["chat"]["id"] = (-43).into(),
            "topic" => value[1]["message_thread_id"] = 8.into(),
            "bot" => value[1]["from"]["id"] = 78.into(),
            "human" => value[1]["from"]["is_bot"] = false.into(),
            "group" => value[1]["media_group_id"] = "different".into(),
            "missing_group" => {
                value[1].as_object_mut().unwrap().remove("media_group_id");
            }
            "media" => {
                value[1].as_object_mut().unwrap().remove("photo");
            }
            "caption" => value[1]["caption"] = "wrong".into(),
            "id" => value[1]["message_id"] = 0.into(),
            "object" => value = value[0].clone(),
            _ => unreachable!(),
        }
        f.answer(answer(value));
        let receipt = send(
            &sender,
            &channel,
            &event,
            &message,
            &RecordingGate::new(true),
        )
        .await;
        assert!(
            matches!(receipt.delivery, DeliveryOutcome::Unknown { .. }),
            "{case}"
        );
        assert!(receipt.message_ref.is_none());
        assert_eq!(f.calls().len(), 1, "{case}");
    }
}

#[tokio::test]
async fn native_telegram_album_never_retries_or_falls_back_after_refusal_or_uncertainty() {
    for case in ["throttle", "reject", "server", "lost", "invalid"] {
        let f = Fixture::new().await;
        let (mut sender, channel, event) = setup(&f);
        sender.relay = sender.relay.with_native_telegram_albums(true);
        let mut response = match case {
            "throttle" => Answer::json(
                reqwest::StatusCode::TOO_MANY_REQUESTS,
                json!({"ok":false,"error_code":429,"parameters":{"retry_after":1}}),
            ),
            "reject" => Answer::json(reqwest::StatusCode::BAD_REQUEST, json!({"ok":false})),
            "server" => Answer::json(reqwest::StatusCode::BAD_GATEWAY, json!({"ok":false})),
            _ => Answer::ok(),
        };
        if case == "lost" {
            response.broken = true;
        }
        if case == "invalid" {
            response.body = b"not json".to_vec();
        }
        f.answer(response);
        let receipt = send(
            &sender,
            &channel,
            &event,
            &album(&[NativeMediaKind::Image; 2]),
            &RecordingGate::new(true),
        )
        .await;
        assert!(!matches!(
            receipt.delivery,
            DeliveryOutcome::Confirmed { .. }
        ));
        assert!(receipt.message_ref.is_none());
        assert_eq!(f.calls().len(), 1, "{case}");
    }
}

#[tokio::test]
async fn native_telegram_album_prepares_every_file_before_text_and_honors_send_gate() {
    for bad_file in [false, true] {
        let f = Fixture::new().await;
        let (mut sender, channel, event) = setup(&f);
        sender.relay = sender.relay.with_native_telegram_albums(true);
        let mut message = album(&[NativeMediaKind::Image; 2]);
        message.text = Some("Only after complete preparation".into());
        if bad_file {
            message.native_attachments[1].source = cmaeg_core::message::NativeMediaSource::Url {
                url: "https://127.0.0.1/private".into(),
            };
        }
        let receipt = send(
            &sender,
            &channel,
            &event,
            &message,
            &RecordingGate::new(false),
        )
        .await;
        assert!(matches!(receipt.delivery, DeliveryOutcome::Rejected { .. }));
        assert!(f.calls().is_empty());
    }
}

#[tokio::test]
async fn native_telegram_album_text_remains_separate_and_revocation_prevents_album() {
    let f = Fixture::new().await;
    let (mut sender, channel, event) = setup(&f);
    sender.relay = sender.relay.with_native_telegram_albums(true);
    let mut message = album(&[NativeMediaKind::File; 2]);
    message.text = Some("Text sent first".into());
    let gate = RevokeAfterFirst(std::sync::atomic::AtomicUsize::new(0));
    let receipt = send(&sender, &channel, &event, &message, &gate).await;
    assert!(
        matches!(receipt.delivery, DeliveryOutcome::Unknown { ref reason } if reason == "partial_delivery")
    );
    assert!(receipt.message_ref.is_none());
    assert_eq!(f.calls().len(), 1);
    assert_eq!(f.calls()[0].path, "/bot77:fixture/sendMessage");
}
