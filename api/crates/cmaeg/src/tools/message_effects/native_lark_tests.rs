use super::{native_lark_fixture::Fixture, *};
use cmaeg_core::{channel::LarkRegion, message::OutboundMessage};
use reqwest::Method;
use serde_json::json;

fn text(value: &str) -> OutboundMessage {
    serde_json::from_value(json!({"text":value})).unwrap()
}
#[tokio::test]
async fn native_lark_discovery_uses_independent_confirmed_and_received_issuance() {
    let mut f = Fixture::new(LarkRegion::Feishu, false, false).await;
    for (confirmed, received) in [(false, false), (false, true), (true, false), (true, true)] {
        f.gates(confirmed, received);
        let context = f.context().await;
        assert_eq!(context.capabilities.edit, confirmed);
        assert_eq!(context.capabilities.message_effects.is_some(), confirmed);
        assert_eq!(context.received_message.is_some(), received);
        let listed = crate::mcp::catalog::view(&f.app.state)
            .iter()
            .any(|t| t.name == "setMessageReaction");
        assert_eq!(listed, confirmed || received);
    }
    assert!(f.writes().is_empty());
}
#[tokio::test]
async fn native_lark_confirmed_reference_requires_its_retained_receipt() {
    let f = Fixture::new(LarkRegion::Feishu, false, false).await;
    let reference = f.confirmed(true).await;
    f.app.backend.expire_at(i64::MAX);
    assert!(f
        .app
        .state
        .message_effect_capabilities(f.app.caller(), &reference)
        .await
        .is_err());
    assert!(f
        .react(&reference, "reaction", Some("SMILE"))
        .await
        .is_err());
    assert!(f.api.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn native_lark_regional_confirmed_text_edit_reaction_clear_and_recall() {
    for region in [LarkRegion::Lark, LarkRegion::Feishu] {
        for named in [false, true] {
            let mut f = Fixture::new(region, named, true).await;
            let reference = f.confirmed(true).await;
            f.gates(false, false);
            let capability = f
                .app
                .state
                .message_effect_capabilities(f.app.caller(), &reference)
                .await
                .unwrap();
            assert!(capability.delete && capability.reaction);
            assert_eq!(capability.scope, "confirmed_native_bot_message");
            let edit = f
                .app
                .state
                .update(f.app.caller(), &reference, text("edited"))
                .await
                .unwrap();
            assert_eq!(edit.delivery.kind(), "confirmed");
            assert_eq!(
                f.react(&reference, "reaction", Some("SMILE"))
                    .await
                    .unwrap()
                    .delivery
                    .kind(),
                "confirmed"
            );
            let payload = reference.open(&f.app.state.keys).unwrap().payload;
            let reminted = MessageRef::seal(&payload, &f.app.state.keys).unwrap();
            assert_eq!(
                f.react(&reminted, "reaction", Some("SMILE"))
                    .await
                    .unwrap()
                    .delivery
                    .kind(),
                "confirmed"
            );
            assert_eq!(
                f.react(&reference, "clear", None)
                    .await
                    .unwrap()
                    .delivery
                    .kind(),
                "confirmed"
            );
            assert_eq!(
                f.app
                    .state
                    .delete_message(f.app.caller(), &reference, "recall")
                    .await
                    .unwrap()
                    .delivery
                    .kind(),
                "confirmed"
            );
            let writes = f.writes();
            assert_eq!(writes.len(), 4);
            assert_eq!(writes[0].0, Method::PUT);
            assert_eq!(writes[0].2["msg_type"], "text");
            assert_eq!(writes[0].2["content"], json!({"text":"edited"}).to_string());
            assert!(writes[2].1.path().ends_with("/reactions/own_reaction"));
            assert!(writes
                .iter()
                .all(|(_, url, _)| url.as_str().starts_with(region.origin())
                    && url.path().contains("/messages/om_sent")));
            assert!(
                f.app.memory.requests.lock().unwrap().is_empty(),
                "native effects never use relay authority"
            );
        }
    }
}
#[tokio::test]
async fn native_lark_received_reactions_are_retained_human_only_and_gate_off_readable() {
    for region in [LarkRegion::Lark, LarkRegion::Feishu] {
        for named in [false, true] {
            let mut f = Fixture::new(region, named, named).await;
            let reference = f.received().await;
            let capability = f
                .app
                .state
                .message_effect_capabilities(f.app.caller(), &reference)
                .await
                .unwrap();
            assert!(!capability.delete && capability.reaction);
            assert_eq!(capability.scope, "received_native_user_message");
            assert_eq!(
                f.app
                    .state
                    .delete_message(f.app.caller(), &reference, "delete")
                    .await
                    .unwrap()
                    .delivery
                    .kind(),
                "unsupported"
            );
            assert_eq!(
                f.app
                    .state
                    .update(f.app.caller(), &reference, text("edit"))
                    .await
                    .unwrap()
                    .delivery
                    .kind(),
                "unsupported"
            );
            f.gates(false, false);
            assert!(f.context().await.received_message.is_none());
            assert!(!f.context().await.capabilities.edit);
            assert_eq!(
                f.react(&reference, "add", Some("SMILE"))
                    .await
                    .unwrap()
                    .delivery
                    .kind(),
                "confirmed"
            );
            assert_eq!(
                f.react(&reference, "replace", Some("THUMBSUP"))
                    .await
                    .unwrap()
                    .delivery
                    .kind(),
                "confirmed"
            );
            assert_eq!(f.writes().len(), 3);
            assert_eq!(
                f.api
                    .calls
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|(_, u, _)| u.path().ends_with("/bot/v3/info"))
                    .count(),
                3
            );
            assert!(
                f.react(&reference, "add", Some("THUMBSUP")).await.is_err(),
                "a key cannot change its action"
            );
        }
    }
}
#[tokio::test]
async fn native_lark_media_is_effect_only_and_unbounded_edits_do_not_dispatch() {
    let f = Fixture::new(LarkRegion::Feishu, false, false).await;
    let reference = f.confirmed(false).await;
    f.api.row.lock().unwrap()["msg_type"] = "file".into();
    assert_eq!(
        f.app
            .state
            .update(f.app.caller(), &reference, text("text"))
            .await
            .unwrap()
            .delivery
            .kind(),
        "unsupported"
    );
    assert_eq!(
        f.app
            .state
            .delete_message(f.app.caller(), &reference, "delete")
            .await
            .unwrap()
            .delivery
            .kind(),
        "confirmed"
    );
    let f = Fixture::new(LarkRegion::Feishu, false, false).await;
    let reference = f.confirmed(true).await;
    assert_eq!(
        f.app
            .state
            .update(f.app.caller(), &reference, text(&"x".repeat(9000)))
            .await
            .unwrap()
            .delivery
            .kind(),
        "unsupported"
    );
    assert!(f.writes().is_empty());
}
#[tokio::test]
async fn native_lark_received_context_requires_exact_retained_original_admission() {
    for change in [
        "event",
        "partition",
        "conversation",
        "consumer",
        "lineage",
        "expired",
    ] {
        let mut f = Fixture::new(LarkRegion::Feishu, false, false).await;
        match change {
            "event" => {
                f.app.event.event_id = "other-event".parse().unwrap();
                f.app.activity.event_id = f.app.event.event_id.clone();
            }
            "partition" => f.app.event.partition_key = "different-partition".into(),
            "conversation" => {
                f.app.event.conversation_id =
                    "conv_00000000000000000000000000000002".parse().unwrap()
            }
            "consumer" => {
                f.app.named_consumer();
            }
            "lineage" => {
                f.app.activity.actor.kind = cmaeg_core::activity::ActorKind::Bot;
            }
            _ => {
                f.app.activity.occurred_at = "2020-01-01T00:00:00Z".into();
            }
        }
        f.app.event.activity_sha256 = Some(cmaeg_core::seal::event_ref::activity_digest(
            &f.app.activity,
        ));
        let result = f
            .app
            .state
            .received_message_context(f.app.caller(), &f.app.event, &f.app.activity, true)
            .await;
        assert!(result.is_none(), "{change}");
        assert!(f.api.calls.lock().unwrap().is_empty());
    }
}
#[tokio::test]
async fn native_lark_strict_metadata_and_reaction_owner_receipts() {
    for change in [
        "chat",
        "message",
        "tenant",
        "sender",
        "type",
        "deleted",
        "thread",
        "missing_deleted",
        "wrong_app_reaction",
        "wrong_emoji_reaction",
        "missing_reaction",
    ] {
        let f = Fixture::new(LarkRegion::Feishu, false, false).await;
        let reference = f.received().await;
        match change {
            "chat" => f.api.row.lock().unwrap()["chat_id"] = "oc_other".into(),
            "message" => f.api.row.lock().unwrap()["message_id"] = "om_other".into(),
            "tenant" => f.api.row.lock().unwrap()["sender"]["tenant_key"] = "other".into(),
            "sender" => f.api.row.lock().unwrap()["sender"]["id"] = "ou_other".into(),
            "type" => f.api.row.lock().unwrap()["sender"]["sender_type"] = "app".into(),
            "deleted" => f.api.row.lock().unwrap()["deleted"] = true.into(),
            "thread" => f.api.row.lock().unwrap()["thread_id"] = json!(null),
            "missing_deleted" => {
                f.api
                    .row
                    .lock()
                    .unwrap()
                    .as_object_mut()
                    .unwrap()
                    .remove("deleted");
            }
            _ => {
                let mut value = json!({"code":0,"data":{"reaction_id":"own_reaction","operator":{"operator_type":"app","operator_id":"cli_native_app"},"reaction_type":{"emoji_type":"SMILE"}}});
                if change == "wrong_app_reaction" {
                    value["data"]["operator"]["operator_id"] = "cli_other".into();
                }
                if change == "wrong_emoji_reaction" {
                    value["data"]["reaction_type"]["emoji_type"] = "THUMBSUP".into();
                }
                if change == "missing_reaction" {
                    value["data"] = json!({});
                }
                f.api.overrides.lock().unwrap().insert(
                    "POST /open-apis/im/v1/messages/om_received/reactions".into(),
                    value,
                );
            }
        }
        let result = f
            .react(&reference, "reaction", Some("SMILE"))
            .await
            .unwrap();
        if change.ends_with("reaction") {
            assert_eq!(result.delivery.kind(), "unknown", "{change}");
            assert_eq!(
                f.react(&reference, "new-key", None)
                    .await
                    .unwrap()
                    .delivery
                    .kind(),
                "unknown"
            );
            assert_eq!(f.writes().len(), 1);
        } else {
            assert_eq!(result.delivery.kind(), "rejected", "{change}");
            assert!(f.writes().is_empty());
        }
    }
}
