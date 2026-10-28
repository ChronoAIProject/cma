use cmaeg_core::{
    activity::Activity,
    channel::{ChannelDefinition, CustomAuth, PartitionKey, PartitionRule, Source},
    cloudevent::{parse_binary, parse_structured, CloudEvent},
    ids::{ConversationId, EventId},
    message::OutboundMessage,
    profile::{canonical_json, EventWorkerProfile},
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
        serde_json::from_value::<EventWorkerProfile>(serde_json::Value::Object(profile)).is_err()
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
        whatsapp_template: None,
        clear_native_attachments: false,
        native_media_update: None,
        native_attachments: Vec::new(),
        thread_reply: false,
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
        attachments: None,
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
            activity.partition_key(&PartitionRule {
                version: 1,
                key,
                custom_key: None
            }),
            expected
        );
    }
}

#[test]
fn canonical_json_golden_vector_is_byte_stable() {
    let value = serde_json::json!({"b":1,"a":[{"d":2,"c":3}]});
    assert_eq!(canonical_json(value), r#"{"a":[{"c":3,"d":2}],"b":1}"#);
}

#[test]
fn relay_bot_username_is_an_optional_bounded_identity_pin() {
    let mut value: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/channel.json")).unwrap();
    value["sources"] = serde_json::json!([{"type":"nyxid_relay", "issuer":"https://nyx.example", "jwks_url":"https://nyx.example/.well-known/jwks.json", "key_id":"key-1", "route_ids":["route-1"], "platform":"telegram", "admission":{"type":"all"}}]);
    value["partition"]["key"] = "conversation".into();
    let profile: EventWorkerProfile = serde_json::from_value(value["profile"].clone()).unwrap();
    value["profile_digest"] = profile.digest().into();
    let channel: ChannelDefinition = serde_json::from_value(value.clone()).unwrap();
    channel.validate().unwrap();
    for username in [
        "",
        "short name",
        "@support_bot",
        "a",
        "abcdefghijklmnopqrstuvwxyz_0123456789",
    ] {
        value["sources"][0]["bot_username"] = username.into();
        assert!(serde_json::from_value::<ChannelDefinition>(value.clone())
            .unwrap()
            .validate()
            .is_err());
    }
    value["sources"][0]["bot_username"] = "support_bot".into();
    serde_json::from_value::<ChannelDefinition>(value)
        .unwrap()
        .validate()
        .unwrap();
}

#[test]
fn scoped_identity_allowlists_have_a_serializable_wire_shape() {
    use cmaeg_core::channel::IdentityAdmission;
    let policy = IdentityAdmission::Allowlist(vec!["sender-1".into()]);
    let json = serde_json::to_value(&policy).unwrap();
    assert_eq!(
        json,
        serde_json::json!({"type":"allowlist", "ids":["sender-1"]})
    );
    assert_eq!(
        serde_json::from_value::<IdentityAdmission>(json).unwrap(),
        policy
    );
    assert_eq!(
        serde_json::to_value(IdentityAdmission::Deny).unwrap(),
        serde_json::json!({"type":"deny"})
    );
}

#[test]
fn clear_native_attachments_requires_an_unambiguous_complete_update() {
    let parse = |value| serde_json::from_value::<OutboundMessage>(value).unwrap();
    assert!(
        parse(serde_json::json!({"text":"Clear files", "clear_native_attachments":true}))
            .validate()
            .is_ok()
    );
    assert!(parse(serde_json::json!({"clear_native_attachments":true}))
        .validate()
        .is_err());
    assert!(parse(
        serde_json::json!({"text":"Clear files", "clear_native_attachments":true,
        "native_attachments":[{"kind":"file","source":{"type":"base64","data":"AQ=="}}]})
    )
    .validate()
    .is_err());
    assert!(parse(
        serde_json::json!({"clear_native_attachments":true,"native_media_update":{"caption":""}})
    )
    .validate()
    .is_err());
    assert!(serde_json::from_value::<OutboundMessage>(
        serde_json::json!({"text":"Clear files","clear_native_attachments":null})
    )
    .is_err());
    let legacy = parse(serde_json::json!({"text":"Keep files","native_attachments":[]}));
    assert!(!legacy.clear_native_attachments);
    assert!(serde_json::to_value(legacy)
        .unwrap()
        .get("clear_native_attachments")
        .is_none());
}
