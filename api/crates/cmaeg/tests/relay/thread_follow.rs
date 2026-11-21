use super::support::*;
use cmaeg_core::{activity::ActivityKind, seal::ReplyAddress, thread::ThreadAddress};
use serde_json::{json, Value};

fn policy(value: &mut Value, chats: &[&str]) {
    value["sources"][0]["bot_id"] = json!("ou_bot");
    value["sources"][0]["admission"] = json!({"type":"scoped","senders":{"type":"open"},"chats":{"type":"open"},"groups":"mention_or_reply_to_bot"});
    value["reply"]["thread_contract"] = json!({"version":1,"follow_chat_ids":chats});
}

#[tokio::test]
async fn opted_lark_and_feishu_bind_verified_thread_and_boolean_mention_evidence() {
    for platform in ["lark", "feishu"] {
        let fixture = Fixture::on(platform, |v| policy(v, &["oc_chat"]), Default::default()).await;
        for (mentions, address, others) in [
            (json!([]), ThreadAddress::NotAddressed, false),
            (
                json!([{"key":"@_user_1","id":{"open_id":"ou_someone"},"name":"Private name"}]),
                ThreadAddress::NotAddressed,
                true,
            ),
            (
                json!([{"key":"@_user_1","id":{"open_id":"ou_someone"}},{"key":"@_all"}]),
                ThreadAddress::NotAddressed,
                false,
            ),
            (
                json!([{"key":"@_user_1","id":{"open_id":"ou_someone"}},{"key":"@_user_2","id":{"open_id":"ou_bot"}}]),
                ThreadAddress::Mention,
                false,
            ),
        ] {
            let mut envelope = Envelope::on(platform);
            envelope.group();
            envelope.lark_message()["mentions"] = mentions;
            envelope.lark_message()["root_id"] = json!("om_root");
            envelope.lark_message()["parent_id"] = json!("om_parent");
            envelope.lark_message()["thread_id"] = json!("omt_thread");
            // A forged normalized hint cannot replace raw, signed platform facts.
            envelope.payload["thread_context"] = json!({"root_id":"forged"});
            let reply: Value = fixture.send(&envelope).await.json().await.unwrap();
            assert_eq!(reply["status"], "accepted", "{reply}");
            let request = fixture.recorder.requests.lock().unwrap().pop().unwrap();
            let thread = request.activity.thread.as_ref().unwrap();
            assert_eq!(thread.root_id.as_deref(), Some("om_root"));
            assert_eq!(thread.parent_message_id.as_deref(), Some("om_parent"));
            assert_eq!(thread.native_thread_id.as_deref(), Some("omt_thread"));
            assert_eq!((thread.address, thread.mentions_others), (address, others));
            assert!(!serde_json::to_string(thread)
                .unwrap()
                .contains("ou_someone"));
            assert!(!serde_json::to_string(thread)
                .unwrap()
                .contains("Private name"));
            assert!(
                matches!(&request.activity.kind,ActivityKind::Message{mentions_bot,..} if *mentions_bot == (address==ThreadAddress::Mention))
            );
            let sealed = request
                .event_ref
                .open(&fixture.state.master_keys, now())
                .unwrap()
                .payload;
            assert!(sealed.verifies_activity(&request.activity));
            assert!(matches!(
                sealed.reply,
                ReplyAddress::NyxidRelay {
                    thread_reply: true,
                    ..
                }
            ));
            let mut forged = request.activity.clone();
            forged.thread.as_mut().unwrap().root_id = Some("other".into());
            assert!(!sealed.verifies_activity(&forged));
        }
    }
}

#[tokio::test]
async fn thread_admission_is_per_chat_and_never_bypasses_sender_or_chat_denials() {
    for (chats, edit, reason) in [
        (vec!["other_chat"], "none", "group_not_addressed"),
        (vec!["oc_chat"], "senders", "sender_denied"),
        (vec!["oc_chat"], "chats", "chat_denied"),
        (vec!["oc_chat"], "groups", "group_denied"),
    ] {
        let fixture = Fixture::on(
            "lark",
            |v| {
                policy(v, &chats);
                if edit == "groups" {
                    v["sources"][0]["admission"][edit] = json!("deny");
                } else if edit != "none" {
                    v["sources"][0]["admission"][edit] = json!({"type":"deny"});
                }
            },
            Default::default(),
        )
        .await;
        let mut envelope = Envelope::on("lark");
        envelope.group();
        let result: Value = fixture.send(&envelope).await.json().await.unwrap();
        assert_eq!(result["reason"], reason);
        assert!(fixture.recorder.requests.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn broadcast_does_not_activate_an_unfollowed_chat_and_legacy_shape_is_unchanged() {
    let fixture = Fixture::on("lark", |v| policy(v, &[]), Default::default()).await;
    let mut envelope = Envelope::on("lark");
    envelope.group();
    envelope.lark_message()["mentions"] = json!([{"key":"@_all"}]);
    let result: Value = fixture.send(&envelope).await.json().await.unwrap();
    assert_eq!(result["reason"], "group_not_addressed");
    let legacy = Fixture::on("lark", |_| {}, Default::default()).await;
    legacy.send(&envelope).await;
    let request = legacy.recorder.requests.lock().unwrap().pop().unwrap();
    assert!(request.activity.thread.is_none());
    assert!(serde_json::to_value(&request.activity)
        .unwrap()
        .get("thread")
        .is_none());
    let event = request
        .event_ref
        .open(&legacy.state.master_keys, now())
        .unwrap()
        .payload;
    assert!(serde_json::to_value(event.reply)
        .unwrap()
        .get("thread_reply")
        .is_none());
}

#[tokio::test]
async fn unknown_ancestry_is_not_fabricated_from_the_parent_and_private_chats_stay_legacy() {
    let fixture = Fixture::on("lark", |v| policy(v, &["oc_chat"]), Default::default()).await;
    let mut envelope = Envelope::on("lark");
    envelope.group();
    envelope.lark_message()["parent_id"] = json!("parent_not_root");
    fixture.send(&envelope).await;
    assert!(fixture
        .recorder
        .requests
        .lock()
        .unwrap()
        .pop()
        .unwrap()
        .activity
        .thread
        .unwrap()
        .root_id
        .is_none());
    fixture.send(&Envelope::on("lark")).await;
    assert!(fixture
        .recorder
        .requests
        .lock()
        .unwrap()
        .pop()
        .unwrap()
        .activity
        .thread
        .is_none());
}

#[tokio::test]
async fn thread_follow_preserves_bot_opt_in_and_never_labels_a_bot_human() {
    let fixture = Fixture::on(
        "lark",
        |value| {
            policy(value, &["oc_chat"]);
            value["sources"][0]["admission"]["bot_messages"] = json!("native_mentions");
        },
        Default::default(),
    )
    .await;
    let mut envelope = Envelope::on("lark");
    envelope.group();
    envelope.payload["raw_platform_data"]["event"]["sender"]["sender_type"] = json!("app");
    envelope.lark_message()["mentions"] = json!([{"key":"@_user_1","id":{"open_id":"ou_bot"}}]);
    let response: Value = fixture.send(&envelope).await.json().await.unwrap();
    assert_eq!(response["status"], "accepted");
    let request = fixture.recorder.requests.lock().unwrap().pop().unwrap();
    assert_eq!(
        request.activity.actor.kind,
        cmaeg_core::activity::ActorKind::Bot
    );
    assert!(request.activity.thread.is_none());
    envelope.lark_message()["mentions"] = json!([]);
    let response: Value = fixture.send(&envelope).await.json().await.unwrap();
    assert_eq!(response["reason"], "group_not_addressed");
}

#[tokio::test]
async fn thread_contract_does_not_weaken_exact_open_id_mention_evidence() {
    let fixture = Fixture::on("lark", |value| policy(value, &[]), Default::default()).await;
    let mut envelope = Envelope::on("lark");
    envelope.group();
    envelope.lark_message()["mentions"] = json!([{"key":"@_user_1","id":{"user_id":"ou_bot"}}]);
    let response: Value = fixture.send(&envelope).await.json().await.unwrap();
    assert_eq!(response["reason"], "group_not_addressed");
    assert!(fixture.recorder.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn followed_native_thread_keeps_lark_media_and_sealed_attachment_facts() {
    use super::lark_media::{lark, resource, IMAGE_KEY};
    let fixture = Fixture::on_with_media(
        "lark",
        |value| policy(value, &["oc_chat"]),
        Default::default(),
    )
    .await;
    let mut envelope = lark(
        "image",
        json!({"image_key":IMAGE_KEY}),
        "image",
        vec![resource(0, "image_key", IMAGE_KEY, json!({}))],
    );
    envelope.group();
    envelope.lark_message()["root_id"] = json!("om_root");
    let response: Value = fixture.send(&envelope).await.json().await.unwrap();
    assert_eq!(response["status"], "accepted", "{response}");
    let request = fixture.recorder.requests.lock().unwrap().pop().unwrap();
    assert_eq!(
        request.activity.thread.as_ref().unwrap().root_id.as_deref(),
        Some("om_root")
    );
    let ActivityKind::Message { attachments, .. } = &request.activity.kind else {
        panic!("expected media message")
    };
    assert_eq!(attachments.len(), 1);
    assert_eq!(
        attachments[0].kind,
        cmaeg_core::activity::AttachmentKind::Image
    );
    let sealed = request
        .event_ref
        .open(&fixture.state.master_keys, now())
        .unwrap()
        .payload;
    assert!(sealed.verifies_activity(&request.activity));
}
