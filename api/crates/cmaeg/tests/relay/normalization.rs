use super::support::*;
use cmaeg_core::{
    activity::{ActorKind, ConversationKind},
    ids::ConversationId,
    seal::ReplyAddress,
};
use serde_json::{json, Value};
use std::sync::atomic::Ordering;

#[tokio::test]
async fn accepts_duplicates_derives_conversation_and_seals_reply_reference() {
    let fixture = Fixture::new(|_| {}, Default::default()).await;
    let envelope = Envelope::new();
    let first: Value = fixture.send(&envelope).await.json().await.unwrap();
    assert_eq!(first["status"], "accepted", "{first}");
    fixture.recorder.outcome.store(1, Ordering::SeqCst);
    let second: Value = fixture.send(&envelope).await.json().await.unwrap();
    assert_eq!(first, second);
    assert_eq!(fixture.backend.gets.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.backend.records.load(Ordering::SeqCst), 1);
    let requests = fixture.recorder.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    let request = &requests[0];
    assert_eq!(request.activity.actor.kind, ActorKind::Human);
    assert_eq!(request.activity.actor.id, "7");
    assert_eq!(
        request.activity.conversation.kind,
        ConversationKind::Private
    );
    assert_eq!(request.activity.conversation.id, "42");
    assert_eq!(request.activity.occurred_at, envelope.payload["timestamp"]);
    assert_eq!(request.idempotency_key.as_str(), "evt_message-1");
    let expected = ConversationId::derive(&fixture.state.master_keys, &fixture.id, 1, "42\u{1f}7");
    assert_eq!(request.conversation_id, expected);
    let opened = request
        .event_ref
        .open(&fixture.state.master_keys, now())
        .unwrap()
        .payload;
    assert_eq!(opened.channel_id, fixture.id);
    assert_eq!(opened.conversation_id, expected);
    assert_eq!(opened.partition_key, "42\u{1f}7");
    assert_eq!(opened.expires_at, envelope.reply["exp"].as_i64().unwrap());
    match opened.reply {
        ReplyAddress::NyxidRelay {
            message_id,
            route_id,
            reply_token,
            reply_token_expires_at,
            ..
        } => {
            assert_eq!(message_id, "message-1");
            assert_eq!(route_id, "route-1");
            assert_eq!(reply_token_expires_at, opened.expires_at);
            assert_eq!(
                reply_token.as_str(),
                super::signer::Signer::shared().token(&envelope.reply)
            );
        }
        _ => panic!("wrong reply transport"),
    }
}
async fn refusal(fixture: &Fixture, envelope: &Envelope, reason: &str) {
    let response = fixture.send(envelope).await;
    assert_eq!(response.status(), 202);
    assert!(response.headers().contains_key("server-timing"));
    let value: Value = response.json().await.unwrap();
    assert_eq!(value, json!({"status":"refused","reason":reason}));
}
#[tokio::test]
async fn every_unsupported_telegram_event_is_an_explicit_refusal() {
    let fixture = Fixture::new(|_| {}, Default::default()).await;
    for (pointer, value, reason) in [
        (
            "/raw_platform_data/message/from/is_bot",
            json!(true),
            "bot_sender",
        ),
        (
            "/raw_platform_data/message/from/is_bot",
            Value::Null,
            "unknown_sender",
        ),
        (
            "/raw_platform_data/message/sender_chat",
            json!({"id":42}),
            "system_sender",
        ),
        (
            "/raw_platform_data/message/new_chat_title",
            json!("new"),
            "system_event",
        ),
        (
            "/raw_platform_data/message/photo",
            json!([]),
            "attachment_unsupported",
        ),
        (
            "/raw_platform_data/message/text",
            json!("different"),
            "content_unsupported",
        ),
        ("/content/type", json!("image"), "content_unsupported"),
        ("/raw_platform_data", Value::Null, "unknown_sender"),
        (
            "/raw_platform_data",
            json!({"callback_query":{"id":"cb-1","data":"ce1:bad:0","from":{"id":100,"is_bot":true}}}),
            "bot_sender",
        ),
        (
            "/raw_platform_data",
            json!({"other_event":{}}),
            "system_event",
        ),
    ] {
        let mut envelope = Envelope::new();
        let (parent, key) = pointer.rsplit_once('/').unwrap();
        envelope.payload.pointer_mut(parent).unwrap()[key] = value;
        refusal(&fixture, &envelope, reason).await;
    }
    for (kind, reason) in [
        ("edited_message", "message_edit"),
        ("edited_channel_post", "message_edit"),
        ("channel_post", "system_sender"),
    ] {
        let mut envelope = Envelope::new();
        let message = envelope.payload["raw_platform_data"]
            .as_object_mut()
            .unwrap()
            .remove("message")
            .unwrap();
        envelope.payload["raw_platform_data"][kind] = message;
        refusal(&fixture, &envelope, reason).await;
    }
    assert!(fixture.recorder.requests.lock().unwrap().is_empty());
}
fn scoped(groups: &str) -> Value {
    json!({"type":"scoped","senders":{"type":"open"},"chats":{"type":"open"},"groups":groups})
}
#[tokio::test]
async fn enforces_sender_chat_group_and_addressing_admission() {
    for (policy, pin, reason) in [
        (
            json!({"type":"allowlist","sender_ids":["other"]}),
            true,
            "sender_denied",
        ),
        (
            json!({"type":"scoped","senders":{"type":"deny"},"chats":{"type":"open"},"groups":"all"}),
            true,
            "sender_denied",
        ),
        (
            json!({"type":"scoped","senders":{"type":"allowlist","ids":["7"]},"chats":{"type":"allowlist","ids":["other"]},"groups":"all"}),
            true,
            "chat_denied",
        ),
        (scoped("deny"), true, "group_denied"),
        (scoped("mention"), true, "group_not_addressed"),
        (scoped("mention"), false, "addressing_evidence_unavailable"),
        (scoped("reply_to_bot"), true, "group_not_addressed"),
    ] {
        let fixture = Fixture::new(
            |value| {
                value["sources"][0]["admission"] = policy;
                if !pin {
                    value["sources"][0]
                        .as_object_mut()
                        .unwrap()
                        .remove("bot_username");
                }
            },
            Default::default(),
        )
        .await;
        let mut envelope = Envelope::new();
        envelope.group();
        refusal(&fixture, &envelope, reason).await;
        assert!(fixture.recorder.requests.lock().unwrap().is_empty());
    }
}
#[tokio::test]
async fn mention_entities_use_pinned_identity_and_utf16_offsets() {
    let fixture = Fixture::new(
        |value| value["sources"][0]["admission"] = scoped("mention"),
        Default::default(),
    )
    .await;
    let mut envelope = Envelope::new();
    envelope.group();
    let text = "🤖 @SuPpOrT_BoT hello";
    envelope.payload["content"]["text"] = text.into();
    envelope.payload["raw_platform_data"]["message"]["text"] = text.into();
    envelope.payload["raw_platform_data"]["message"]["entities"] =
        json!([{"type":"mention","offset":3,"length":12}]);
    assert_eq!(
        fixture.send(&envelope).await.json::<Value>().await.unwrap()["status"],
        "accepted"
    );
    envelope.payload["raw_platform_data"]["message"]["entities"] = json!([]);
    refusal(&fixture, &envelope, "group_not_addressed").await;
    envelope.payload["raw_platform_data"]["message"]["entities"] =
        json!([{"type":"mention","offset":1,"length":12}]);
    problem(fixture.send(&envelope).await, 401, "telegram_entities").await;
}
#[tokio::test]
async fn reply_gating_and_topic_partition_require_raw_evidence() {
    let fixture = Fixture::new(
        |value| {
            value["sources"][0]["admission"] = scoped("reply_to_bot");
            value["partition"]["key"] = "thread".into();
        },
        Default::default(),
    )
    .await;
    let mut envelope = Envelope::new();
    envelope.group();
    envelope.payload["raw_platform_data"]["message"]["reply_to_message"] = json!({"from":{}});
    refusal(&fixture, &envelope, "addressing_evidence_unavailable").await;
    envelope.payload["raw_platform_data"]["message"]["reply_to_message"]["from"]["is_bot"] =
        true.into();
    refusal(&fixture, &envelope, "thread_evidence_unavailable").await;
    envelope.payload["thread_id"] = "unverified-context".into();
    envelope.payload["raw_platform_data"]["message"]["message_thread_id"] = 17.into();
    refusal(&fixture, &envelope, "thread_evidence_unavailable").await;
    envelope.payload["raw_platform_data"]["message"]["is_topic_message"] = true.into();
    assert_eq!(
        fixture.send(&envelope).await.json::<Value>().await.unwrap()["status"],
        "accepted"
    );
    assert_eq!(
        fixture.recorder.requests.lock().unwrap()[0]
            .activity
            .conversation
            .thread_id
            .as_deref(),
        Some("17")
    );
}

#[tokio::test]
async fn external_native_bot_mentions_reach_the_provider_as_bots_only_after_opt_in() {
    for platform in ["telegram", "lark", "feishu"] {
        let receiver = if platform == "telegram" {
            "77"
        } else {
            "ou_receiver"
        };
        let fixture=Fixture::on(platform,|v|{
            v["sources"][0]["bot_id"]=json!(receiver);
            v["sources"][0]["admission"]=json!({"type":"scoped","senders":{"type":"open"},"chats":{"type":"open"},"groups":"all","bot_messages":"native_mentions"});
        },Default::default()).await;
        fixture.recorder.outcome.store(1, Ordering::SeqCst);
        let mut envelope = Envelope::on(platform);
        envelope.group();
        envelope.payload["timestamp"] = json!("2026-10-15T00:00:00Z");
        if platform == "telegram" {
            envelope.payload["content"]["text"] = json!("😀 /ask@support_bot");
            let m = &mut envelope.payload["raw_platform_data"]["message"];
            m["from"]["is_bot"] = json!(true);
            m["text"] = json!("😀 /ask@support_bot");
            m["entities"] = json!([{"type":"bot_command","offset":3,"length":16}]);
        } else {
            envelope.payload["raw_platform_data"]["event"]["sender"]["sender_type"] = json!("app");
            envelope.lark_text("@_user_1 hello");
            envelope.lark_message()["mentions"] =
                json!([{"key":"@_user_1","id":{"open_id":receiver},"name":"Support"}]);
        }
        let response: Value = fixture.send(&envelope).await.json().await.unwrap();
        assert_eq!(response["status"], "accepted", "{platform}: {response}");
        {
            let requests = fixture.recorder.requests.lock().unwrap();
            assert_eq!(requests.len(), 1);
            let activity = serde_json::to_value(&requests[0].activity).unwrap();
            assert_eq!(activity["actor"]["kind"], "bot");
            assert_eq!(activity["kind"]["mentions_bot"], true);
            assert_eq!(activity["conversation"]["kind"], "group");
            let expected: Value = serde_json::from_str(match platform {
                "telegram" => include_str!("../fixtures/native_bot_mentions/telegram.json"),
                "lark" => include_str!("../fixtures/native_bot_mentions/lark.json"),
                _ => include_str!("../fixtures/native_bot_mentions/feishu.json"),
            })
            .unwrap();
            assert_eq!(activity, expected, "shared NyxID provider wire fixture");
            if let Ok(directory) = std::env::var("NYXID_MENTIONS_ACTIVITY_ARTIFACT_DIR") {
                std::fs::create_dir_all(&directory).unwrap();
                std::fs::write(
                    std::path::Path::new(&directory).join(format!("{platform}.json")),
                    serde_json::to_vec_pretty(&activity).unwrap(),
                )
                .unwrap();
            }
        }
        // `all` never authorizes unaddressed bot traffic.
        if platform == "telegram" {
            envelope.payload["raw_platform_data"]["message"]["entities"] = json!([]);
        } else {
            envelope.lark_message()["mentions"] = json!([]);
        }
        let response: Value = fixture.send(&envelope).await.json().await.unwrap();
        assert_eq!(response["status"], "refused");
        assert_eq!(fixture.recorder.requests.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn external_bot_opt_in_never_admits_self_private_reply_only_or_unknown_actors() {
    for platform in ["telegram", "lark", "feishu"] {
        let receiver = if platform == "telegram" {
            "77"
        } else {
            "ou_receiver"
        };
        for case in [
            "default",
            "self",
            "private",
            "unaddressed",
            "unknown",
            "broadcast",
            "reply",
            "deny_sender",
            "deny_chat",
        ] {
            let fixture = Fixture::on(
                platform,
                |v| {
                    v["sources"][0]["bot_id"] = json!(receiver);
                    let mut policy = scoped("all");
                    if case != "default" {
                        policy["bot_messages"] = json!("native_mentions");
                    }
                    if case == "deny_sender" {
                        policy["senders"] = json!({"type":"deny"});
                    }
                    if case == "deny_chat" {
                        policy["chats"] = json!({"type":"deny"});
                    }
                    v["sources"][0]["admission"] = policy;
                },
                Default::default(),
            )
            .await;
            let mut e = Envelope::on(platform);
            if case != "private" {
                e.group();
            }
            if platform == "telegram" {
                e.payload["content"]["text"] = json!("😀 @support_bot");
                let m = &mut e.payload["raw_platform_data"]["message"];
                m["text"] = json!("😀 @support_bot");
                m["entities"] = json!([{"type":"mention","offset":3,"length":12}]);
                m["from"]["is_bot"] = if case == "unknown" {
                    Value::Null
                } else {
                    json!(true)
                };
                if case == "self" {
                    m["from"]["id"] = json!(77);
                    e.payload["sender"]["platform_id"] = json!("77");
                }
                if matches!(case, "unaddressed" | "broadcast" | "reply") {
                    e.payload["raw_platform_data"]["message"]["entities"] = json!([]);
                }
                if case == "reply" {
                    e.payload["raw_platform_data"]["message"]["reply_to_message"] =
                        json!({"from":{"id":77,"is_bot":true}});
                }
            } else {
                e.lark_text("@_user_1 hello");
                e.payload["raw_platform_data"]["event"]["sender"]["sender_type"] =
                    json!(if case == "unknown" { "system" } else { "app" });
                e.lark_message()["mentions"] =
                    json!([{"key":"@_user_1","id":{"open_id":receiver},"name":"Support"}]);
                if case == "self" {
                    e.payload["raw_platform_data"]["event"]["sender"]["sender_id"]["open_id"] =
                        json!(receiver);
                    e.payload["sender"]["platform_id"] = json!(receiver);
                }
                if matches!(case, "unaddressed" | "reply") {
                    e.lark_message()["mentions"] = json!([]);
                }
                if case == "broadcast" {
                    e.lark_message()["mentions"] = json!([{"key":"@_all","id":{"open_id":"all"}}]);
                }
                if case == "reply" {
                    e.lark_message()["parent_id"] = json!("om_parent");
                }
            }
            let response: Value = fixture.send(&e).await.json().await.unwrap();
            assert_eq!(
                response["status"], "refused",
                "{platform}/{case}: {response}"
            );
            assert!(
                fixture.recorder.requests.lock().unwrap().is_empty(),
                "{platform}/{case}"
            );
        }
    }
}
