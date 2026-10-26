use cmaeg_core::channel::ChannelDefinition;
use serde_json::{json, Value};
fn definition() -> Value {
    let mut v: Value = serde_json::from_str(include_str!("fixtures/channel.json")).unwrap();
    v["sources"] = json!([{"type":"nyxid_relay","issuer":"https://nyx.example","jwks_url":"https://nyx.example/.well-known/jwks.json","key_id":"key","route_ids":[],"platform":"lark","bot_id":"ou_bot","admission":{"type":"all"}}]);
    v["partition"]["key"] = json!("conversation_and_sender");
    v["profile_digest"] = json!(serde_json::from_value::<ChannelDefinition>(v.clone())
        .unwrap()
        .profile
        .digest());
    v
}
#[test]
fn thread_contract_requires_proven_platform_identity_and_bounds_follow_chats() {
    let mut base = definition();
    base["reply"]["thread_contract"] = json!({"version":1,"follow_chat_ids":["chat"]});
    serde_json::from_value::<ChannelDefinition>(base.clone())
        .unwrap()
        .validate()
        .unwrap();
    for (pointer, value) in [
        ("/sources/0/bot_id", Value::Null),
        ("/sources/0/platform", json!("telegram")),
        ("/reply/thread_contract/version", json!(2)),
        (
            "/reply/thread_contract/follow_chat_ids",
            json!(["chat", "chat"]),
        ),
        (
            "/reply/thread_contract/follow_chat_ids",
            json!(["bad\nchat"]),
        ),
        (
            "/reply/thread_contract/follow_chat_ids",
            json!((0..257).map(|i| format!("chat{i}")).collect::<Vec<_>>()),
        ),
    ] {
        let mut v = base.clone();
        *v.pointer_mut(pointer).unwrap() = value;
        assert!(
            serde_json::from_value::<ChannelDefinition>(v)
                .unwrap()
                .validate()
                .is_err(),
            "{pointer}"
        );
    }
}
#[test]
fn legacy_policy_and_message_omit_additive_contract_fields() {
    let v = definition();
    let channel: ChannelDefinition = serde_json::from_value(v).unwrap();
    assert!(serde_json::to_value(channel.reply)
        .unwrap()
        .get("thread_contract")
        .is_none());
    let message: cmaeg_core::message::OutboundMessage =
        serde_json::from_value(json!({"text":"legacy"})).unwrap();
    assert!(!message.thread_reply);
    assert!(serde_json::to_value(message)
        .unwrap()
        .get("thread_reply")
        .is_none());
}
