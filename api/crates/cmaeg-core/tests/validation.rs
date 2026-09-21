use cmaeg_core::{
    activity::Activity,
    channel::{ChannelDefinition, CustomAuth, PartitionKey, PartitionRule, Source},
    cloudevent::{parse_binary, parse_structured, CloudEvent},
    ids::{ConversationId, EventId},
    message::OutboundMessage,
    profile::{canonical_json, AgentProfileSnapshot},
};

fn event() -> CloudEvent {
    serde_json::from_str(include_str!("fixtures/cloudevent.json")).expect("event fixture")
}

#[test]
fn rejects_empty_identifiers() {
    assert!("".parse::<EventId>().is_err());
}

#[test]
fn rejects_malformed_conversation_id() {
    let id: ConversationId = "conv_not_hex".parse().expect("bounded id");
    assert!(!id.is_wellformed());
}

#[test]
fn rejects_unknown_profile_fields() {
    let channel: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/channel.json")).unwrap();
    let mut profile = channel["profile"].as_object().unwrap().clone();
    profile.insert("unexpected".into(), serde_json::json!(true));
    assert!(
        serde_json::from_value::<AgentProfileSnapshot>(serde_json::Value::Object(profile)).is_err()
    );
}

#[test]
fn rejects_cloud_event_wrong_version() {
    let mut value = serde_json::to_value(event()).expect("event");
    value["specversion"] = serde_json::json!("0.3");
    let parsed: CloudEvent = serde_json::from_value(value).expect("shape");
    assert!(parsed.validate().is_err());
}

#[test]
fn rejects_cloud_event_unknown_extension_name() {
    let mut value = serde_json::to_value(event()).expect("event");
    value["Bad"] = serde_json::json!(true);
    let parsed: CloudEvent = serde_json::from_value(value).expect("shape");
    assert!(parsed.validate().is_err());
}

#[test]
fn rejects_binary_cloud_event_without_required_headers() {
    assert!(parse_binary(&[], b"{}").is_err());
}

#[test]
fn rejects_structured_cloud_event_too_large() {
    let body = vec![b' '; 256 * 1_024 + 1];
    assert!(parse_structured(&body).is_err());
}

#[test]
fn rejects_empty_outbound_message() {
    assert!(OutboundMessage {
        text: None,
        blocks: vec![],
        reply_to: None
    }
    .validate()
    .is_err());
}

#[test]
fn rejects_duplicate_source_and_partition_mismatch() {
    let source = Source::Custom {
        auth: CustomAuth::Hmac { key_id: "k".into() },
        prompt_template: None,
        schema: None,
    };
    let mut value =
        serde_json::from_str::<ChannelDefinition>(include_str!("fixtures/channel.json"))
            .expect("channel");
    value.sources = vec![source.clone(), source];
    assert!(value.validate().is_err());
}

#[test]
fn activity_partition_keys_cover_each_rule() {
    let activity: Activity =
        serde_json::from_str(include_str!("fixtures/activity.json")).expect("activity");
    for (key, expected) in [
        (PartitionKey::ConversationAndSender, "chat-1\u{1f}user-1"),
        (PartitionKey::Conversation, "chat-1"),
        (PartitionKey::Thread, "chat-1\u{1f}"),
        (PartitionKey::Fixed, "fixed"),
        (PartitionKey::EventSubject, "chat-1"),
        (PartitionKey::PerEvent, "msg-1"),
    ] {
        assert_eq!(
            activity.partition_key(&PartitionRule { version: 1, key }),
            expected
        );
    }
}

#[test]
fn canonical_json_golden_vector_is_byte_stable() {
    let value = serde_json::json!({"b":1,"a":[{"d":2,"c":3}]});
    assert_eq!(canonical_json(value), r#"{"a":[{"c":3,"d":2}],"b":1}"#);
}
