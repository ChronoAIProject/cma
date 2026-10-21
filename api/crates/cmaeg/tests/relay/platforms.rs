use super::{nyxid_shapes::attachment, support::*};
use cmaeg_core::{
    activity::{ActivityKind, ActorKind, ConversationKind, SourceKind},
    channel::Platform,
    seal::ReplyAddress,
};
use serde_json::{json, Value};

async fn status(fixture: &Fixture, envelope: &Envelope) -> Value {
    let response = fixture.send(envelope).await;
    let status = response.status();
    let body: Value = response.json().await.unwrap();
    assert_eq!(status, 202, "{body}");
    body
}
async fn accepted(fixture: &Fixture, envelope: &Envelope) {
    let value = status(fixture, envelope).await;
    assert_eq!(value["status"], "accepted", "{value}");
}
async fn refused(fixture: &Fixture, envelope: &Envelope, reason: &str) {
    assert_eq!(
        status(fixture, envelope).await,
        json!({"status":"refused","reason":reason})
    );
}
fn scoped(groups: &str) -> Value {
    json!({"type":"scoped","senders":{"type":"open"},"chats":{"type":"open"},"groups":groups})
}
fn mention(open_id: &str) -> Value {
    json!([{"key":"@_user_1","id":{"open_id":open_id,"union_id":"on_other"},"name":"Someone"}])
}

#[tokio::test]
async fn lark_and_feishu_messages_become_human_activities_on_their_own_platform() {
    for (wire, platform) in [("lark", Platform::Lark), ("feishu", Platform::Feishu)] {
        let fixture = Fixture::on(wire, |_| {}, Default::default()).await;
        accepted(&fixture, &Envelope::on(wire)).await;
        let mut group = Envelope::on(wire);
        group.group();
        group.lark_text("@_user_1 status?");
        group.lark_message()["mentions"] = mention("ou_bot");
        accepted(&fixture, &group).await;
        let requests = fixture.recorder.requests.lock().unwrap();
        for (request, kind, text, mentioned) in [
            (&requests[0], ConversationKind::Private, "hello", false),
            (
                &requests[1],
                ConversationKind::Group,
                "@_user_1 status?",
                true,
            ),
        ] {
            let activity = &request.activity;
            assert_eq!(activity.actor.kind, ActorKind::Human);
            assert_eq!(activity.actor.id, "ou_sender");
            assert_eq!(activity.conversation.id, "oc_chat");
            assert_eq!(activity.conversation.kind, kind);
            assert_eq!(
                activity.source,
                SourceKind::NyxidRelay {
                    platform,
                    route_id: "route-1".into()
                }
            );
            let ActivityKind::Message {
                text: actual,
                mentions_bot,
                ..
            } = &activity.kind
            else {
                panic!("expected a message");
            };
            assert_eq!((actual.as_str(), *mentions_bot), (text, mentioned));
            // The sealed reply reference carries the platform, and binds this activity.
            let opened = request
                .event_ref
                .open(&fixture.state.master_keys, now())
                .unwrap()
                .payload;
            assert!(opened.verifies_activity(activity));
            let ReplyAddress::NyxidRelay {
                platform: sealed, ..
            } = opened.reply
            else {
                panic!("wrong reply transport");
            };
            assert_eq!(sealed, platform);
        }
    }
}

#[tokio::test]
async fn lark_mentions_use_the_pinned_bot_open_id_or_any_mention_without_a_pin() {
    for (pin, other_mention_admitted) in [(true, false), (false, true)] {
        let fixture = Fixture::on(
            "lark",
            |value| {
                value["sources"][0]["admission"] = scoped("mention");
                if pin {
                    value["sources"][0]["bot_id"] = "ou_bot".into();
                }
            },
            Default::default(),
        )
        .await;
        let mut envelope = Envelope::on("lark");
        envelope.group();
        refused(&fixture, &envelope, "group_not_addressed").await;
        envelope.lark_message()["mentions"] = mention("ou_other");
        if other_mention_admitted {
            accepted(&fixture, &envelope).await;
        } else {
            refused(&fixture, &envelope, "group_not_addressed").await;
        }
        envelope.lark_message()["mentions"] = mention("ou_bot");
        accepted(&fixture, &envelope).await;
        envelope.lark_message()["mentions"] = json!({"not":"a list"});
        problem(fixture.send(&envelope).await, 401, "lark_event_invalid").await;
    }
}

#[tokio::test]
async fn lark_replies_are_not_evidence_that_the_bot_was_addressed() {
    let fixture = Fixture::on(
        "lark",
        |value| {
            value["sources"][0]["admission"] = scoped("reply_to_bot");
            value["sources"][0]["bot_id"] = "ou_bot".into();
        },
        Default::default(),
    )
    .await;
    let mut envelope = Envelope::on("lark");
    envelope.group();
    refused(&fixture, &envelope, "group_not_addressed").await;
    envelope.lark_message()["parent_id"] = "om_parent".into();
    refused(&fixture, &envelope, "addressing_evidence_unavailable").await;
    let fixture = Fixture::on(
        "lark",
        |value| {
            value["sources"][0]["admission"] = scoped("mention_or_reply_to_bot");
            value["sources"][0]["bot_id"] = "ou_bot".into();
        },
        Default::default(),
    )
    .await;
    refused(&fixture, &envelope, "addressing_evidence_unavailable").await;
    envelope.lark_message()["mentions"] = mention("ou_bot");
    accepted(&fixture, &envelope).await;
}

#[tokio::test]
async fn lark_identities_must_match_the_normalized_envelope() {
    let fixture = Fixture::on("lark", |_| {}, Default::default()).await;
    for (pointer, value) in [
        (
            "/raw_platform_data/event/message/chat_id",
            json!("oc_other"),
        ),
        (
            "/raw_platform_data/event/sender/sender_id/open_id",
            json!("ou_other"),
        ),
        (
            "/raw_platform_data/event/sender/sender_id/open_id",
            Value::Null,
        ),
    ] {
        let mut envelope = Envelope::on("lark");
        *envelope.payload.pointer_mut(pointer).unwrap() = value;
        problem(fixture.send(&envelope).await, 401, "lark_identity_mismatch").await;
    }
    let mut envelope = Envelope::on("lark");
    envelope.lark_message()["chat_id"] = 42.into();
    problem(fixture.send(&envelope).await, 401, "lark_event_invalid").await;
    assert!(fixture.recorder.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn every_unsupported_lark_event_is_an_explicit_refusal() {
    let fixture = Fixture::on("lark", |_| {}, Default::default()).await;
    for (pointer, value, reason) in [
        (
            "/raw_platform_data/event/sender/sender_type",
            json!("app"),
            "bot_sender",
        ),
        (
            "/raw_platform_data/event/sender/sender_type",
            Value::Null,
            "unknown_sender",
        ),
        (
            "/raw_platform_data/event/message/message_type",
            json!("image"),
            "attachment_unsupported",
        ),
        (
            "/raw_platform_data/event/message/message_type",
            json!("file"),
            "attachment_unsupported",
        ),
        (
            "/raw_platform_data/event/message/message_type",
            json!("post"),
            "content_unsupported",
        ),
        (
            "/raw_platform_data/event/message/content",
            json!("{\"text\":\"other\"}"),
            "content_unsupported",
        ),
        (
            "/raw_platform_data/event/message/content",
            json!("not json"),
            "content_unsupported",
        ),
        (
            "/raw_platform_data/event/message/chat_type",
            json!("topic_group"),
            "system_event",
        ),
        (
            "/raw_platform_data/header/event_type",
            json!("im.message.message_read_v1"),
            "system_event",
        ),
        ("/content/type", json!("image"), "content_unsupported"),
        ("/raw_platform_data", Value::Null, "unknown_sender"),
    ] {
        let mut envelope = Envelope::on("lark");
        *envelope.payload.pointer_mut(pointer).unwrap() = value;
        refused(&fixture, &envelope, reason).await;
    }
    // NyxID attaches a Lark resource handle; the raw text event alone cannot admit it.
    let mut envelope = Envelope::on("lark");
    envelope.payload["content"]["attachments"] = json!([attachment(
        0,
        "image",
        "https://open.larksuite.com/open-apis/im/v1/messages/om_message/resources/img_v3_photo?type=image",
        json!({"platform_message_id": "om_message", "image_key": "img_v3_photo"}),
    )]);
    refused(&fixture, &envelope, "attachment_unsupported").await;
    assert!(fixture.recorder.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn lark_topic_threads_partition_only_on_the_raw_thread_id() {
    let fixture = Fixture::on(
        "lark",
        |value| value["partition"]["key"] = "thread".into(),
        Default::default(),
    )
    .await;
    let mut envelope = Envelope::on("lark");
    envelope.group();
    envelope.lark_message()["mentions"] = mention("ou_bot");
    refused(&fixture, &envelope, "thread_evidence_unavailable").await;
    envelope.payload["thread_id"] = "unverified-context".into();
    refused(&fixture, &envelope, "thread_evidence_unavailable").await;
    envelope.lark_message()["thread_id"] = "omt_topic".into();
    accepted(&fixture, &envelope).await;
    assert_eq!(
        fixture.recorder.requests.lock().unwrap()[0]
            .activity
            .conversation
            .thread_id
            .as_deref(),
        Some("omt_topic")
    );
}

#[tokio::test]
async fn relay_platform_spellings_select_only_their_own_source() {
    let fixture = Fixture::on("feishu", |_| {}, Default::default()).await;
    for wire in ["lark", "telegram", "Feishu"] {
        let (body, token) = Envelope::on("feishu").signed();
        let mut request = fixture.request(body, &token).build().unwrap();
        request
            .headers_mut()
            .insert("x-nyxid-platform", wire.parse().unwrap());
        problem(
            fixture.client.execute(request).await.unwrap(),
            400,
            "no matching relay source",
        )
        .await;
    }
    // A Lark-stamped callback cannot be replayed onto the Feishu source.
    let mut envelope = Envelope::on("lark");
    envelope.payload["platform"] = "feishu".into();
    problem(fixture.send(&envelope).await, 401, "platform_mismatch").await;
    let lark = Fixture::on("lark", |_| {}, Default::default()).await;
    accepted(&lark, &Envelope::on("lark")).await;
    let telegram = Fixture::on("telegram", |_| {}, Default::default()).await;
    let (body, token) = Envelope::on("telegram-new").signed();
    let mut request = telegram.request(body, &token).build().unwrap();
    request
        .headers_mut()
        .insert("x-nyxid-platform", "telegram-new".parse().unwrap());
    assert_eq!(
        telegram.client.execute(request).await.unwrap().status(),
        202
    );
}

#[tokio::test]
async fn platforms_without_a_verifier_require_an_explicit_trust_opt_in() {
    for wire in ["discord", "slack", "whatsapp"] {
        // Management refuses to create or update such a source; this fixture stands
        // in for a definition stored before that check, which ingress still refuses.
        let fixture = Fixture::on(wire, |_| {}, Default::default()).await;
        refused(
            &fixture,
            &Envelope::on(wire),
            "platform_verifier_unavailable",
        )
        .await;
        assert!(fixture.recorder.requests.lock().unwrap().is_empty());
        let trusted = Fixture::on(
            wire,
            |value| value["sources"][0]["trust_normalized"] = true.into(),
            Default::default(),
        )
        .await;
        for (kind, expected) in [
            ("private", ConversationKind::Private),
            ("group", ConversationKind::Group),
            ("channel", ConversationKind::Channel),
        ] {
            let mut envelope = Envelope::on(wire);
            envelope.payload["conversation"]["type"] = kind.into();
            accepted(&trusted, &envelope).await;
            let request = trusted.recorder.requests.lock().unwrap().pop().unwrap();
            let activity = &request.activity;
            assert_eq!(activity.conversation.kind, expected);
            assert_eq!(activity.conversation.thread_id, None);
            assert_eq!(activity.actor.kind, ActorKind::Unknown);
            assert_eq!(activity.actor.id, "7");
            assert!(matches!(
                activity.kind,
                ActivityKind::Message {
                    mentions_bot: false,
                    ..
                }
            ));
            assert_eq!(
                activity.source,
                SourceKind::NyxidRelay {
                    platform: wire.parse().unwrap(),
                    route_id: "route-1".into()
                }
            );
        }
        for (pointer, value, reason) in [
            ("/conversation/type", json!("dm"), "system_event"),
            ("/content/type", json!("image"), "content_unsupported"),
            ("/content/text", Value::Null, "content_unsupported"),
            (
                "/content/attachments",
                json!([attachment(
                    0,
                    "image",
                    "https://files.example/image.png",
                    json!({"filename": "image.png", "mime_type": "image/png", "size_bytes": 1024}),
                )]),
                "attachment_unsupported",
            ),
        ] {
            let mut envelope = Envelope::on(wire);
            let (parent, key) = pointer.rsplit_once('/').unwrap();
            envelope.payload.pointer_mut(parent).unwrap()[key] = value;
            refused(&trusted, &envelope, reason).await;
        }
    }
}

#[tokio::test]
async fn trusted_platforms_have_no_addressing_or_thread_evidence() {
    let fixture = Fixture::on(
        "slack",
        |value| {
            value["sources"][0]["trust_normalized"] = true.into();
            value["sources"][0]["admission"] = scoped("mention_or_reply_to_bot");
        },
        Default::default(),
    )
    .await;
    let mut envelope = Envelope::on("slack");
    envelope.group();
    refused(&fixture, &envelope, "addressing_evidence_unavailable").await;
    let fixture = Fixture::on(
        "slack",
        |value| {
            value["sources"][0]["trust_normalized"] = true.into();
            value["partition"]["key"] = "thread".into();
        },
        Default::default(),
    )
    .await;
    envelope.payload["thread_id"] = "1700000000.000100".into();
    refused(&fixture, &envelope, "thread_evidence_unavailable").await;
}
