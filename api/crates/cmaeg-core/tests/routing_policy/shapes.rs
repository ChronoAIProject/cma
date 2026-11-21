//! The stored shapes: what a routed definition holds, what its blob and its
//! public view carry, and that every optional field keeps its bytes.
use cmaeg_core::{
    channel::{
        ActivityKindName, Addressing, AgentEventPolicy, ChannelDefinition, DefaultReply, Pattern,
        ReplyLabel, RoutingPolicy, RuleMatch,
    },
    profile::canonical_json,
};
use serde_json::{json, Value};

use super::fixture::{agent_key, definition, routed, NYX, REVIEWER, ROUTE};

/// A policy that sets every optional field away from its default.
fn everything() -> Value {
    let mut routing = routed()["routing"].clone();
    routing["default_consumer"] = json!({"name": "lead", "label": "Lead", "mentionable": false,
        "non_human": true, "reply": {"auto_final": false, "destination": "default"},
        "publish": {"enabled": true, "targets": [REVIEWER]}});
    routing["consumers"][1]["pending"] = true.into();
    routing["rules"] = json!([{"id": "lark-ops", "when": {"sources": ["relay_lark"],
        "kinds": ["message", "command", "interaction", "event"], "chats": ["oc_1"],
        "senders": ["ou_1"], "event_types": ["com.example.*"], "event_sources": ["urn:ci"]},
        "targets": ["default", REVIEWER]}]);
    routing["addressing"] = json!({"mentions": false, "command": false, "event_field": false,
        "unknown_target_notice": false});
    routing["max_targets"] = 2.into();
    routing["reply_label"] = "always".into();
    routing["default_reply"] = json!({"type": "relay_conversation", "source_id": "relay_lark",
        "route_id": ROUTE});
    routing["agent_events"] = json!({"max_hops": 4, "per_consumer_per_minute": 5,
        "per_channel_per_minute": 20});
    routing
}

#[test]
fn a_policy_at_its_defaults_spells_out_only_its_unskipped_defaults() {
    let stored = routed()["routing"].clone();
    let policy: RoutingPolicy = serde_json::from_value(stored.clone()).unwrap();
    assert_eq!(policy.addressing, Addressing::default());
    assert!(policy.addressing.mentions && policy.addressing.unknown_target_notice);
    assert_eq!(policy.max_targets, 4);
    assert_eq!(policy.reply_label, ReplyLabel::Auto);
    assert_eq!(policy.agent_events, AgentEventPolicy::default());
    assert_eq!(
        (
            policy.agent_events.max_hops,
            policy.agent_events.per_consumer_per_minute,
            policy.agent_events.per_channel_per_minute
        ),
        (2, 10, 30)
    );
    let mut expected = stored;
    expected["addressing"] = json!({"mentions": true, "command": true, "event_field": true,
        "unknown_target_notice": true});
    expected["max_targets"] = 4.into();
    expected["reply_label"] = "auto".into();
    assert_eq!(serde_json::to_value(&policy).unwrap(), expected);
}

#[test]
fn every_optional_field_round_trips() {
    let stored = everything();
    let policy: RoutingPolicy = serde_json::from_value(stored.clone()).unwrap();
    assert_eq!(serde_json::to_value(&policy).unwrap(), stored);
    for reply in [
        json!({"type": "telegram_chat", "source_id": "telegram", "chat_id": "-100", "thread_id": "7"}),
        json!({"type": "telegram_chat", "source_id": "telegram", "chat_id": "42"}),
        json!({"type": "sinks"}),
    ] {
        let parsed: DefaultReply = serde_json::from_value(reply.clone()).unwrap();
        assert_eq!(serde_json::to_value(parsed).unwrap(), reply);
    }
    let empty: RuleMatch = serde_json::from_value(json!({})).unwrap();
    assert_eq!(empty, RuleMatch::default());
}

/// `everything()` with the member `key` of the value at `parent` set to `value`.
fn edited(parent: &str, key: &str, value: Value) -> Value {
    let mut routing = everything();
    match routing.pointer_mut(parent).unwrap() {
        Value::Object(object) => {
            object.insert(key.to_owned(), value);
        }
        Value::Array(list) => list[key.parse::<usize>().unwrap()] = value,
        other => panic!("{parent} is not a container: {other}"),
    }
    routing
}

#[test]
fn every_shape_refuses_unknown_fields_and_values() {
    for (parent, key, value) in [
        ("", "unknown", json!(1)),
        ("/rules/0", "unknown", json!(1)),
        ("/rules/0/when", "unknown", json!(1)),
        ("/addressing", "unknown", json!(1)),
        ("/agent_events", "unknown", json!(1)),
        ("/default_reply", "unknown", json!(1)),
        ("/default_consumer", "unknown", json!(1)),
        ("/default_consumer/reply", "unknown", json!(1)),
        ("/default_consumer/publish", "unknown", json!(1)),
        ("", "reply_label", json!("sometimes")),
        ("/default_reply", "type", json!("email")),
        ("/default_targets", "0", json!("reviewer")),
        ("/rules/0", "id", json!("Lark Ops")),
        ("/rules/0/when/kinds", "0", json!("reaction")),
        ("/rules/0/when/sources", "0", json!("relay_teams")),
        ("/rules/0/when/event_types", "0", json!("com.*.x")),
        ("/default_consumer", "name", json!("Lead")),
        ("", "max_targets", json!(256)),
    ] {
        assert!(
            serde_json::from_value::<RoutingPolicy>(edited(parent, key, value)).is_err(),
            "{parent}/{key}"
        );
    }
}

#[test]
fn patterns_are_exact_or_end_in_one_star() {
    for value in ["a", "com.example.*", "*", "urn:ci", &"p".repeat(512)] {
        assert_eq!(Pattern::parse(value).unwrap().as_str(), value);
    }
    for value in ["", "**", "a*b", "*a", "com.*.*", &"p".repeat(513)] {
        assert_eq!(Pattern::parse(value).unwrap_err().pointer, "/", "{value:?}");
    }
    let kinds: Vec<ActivityKindName> =
        serde_json::from_value(json!(["message", "command", "interaction", "event"])).unwrap();
    assert_eq!(
        kinds,
        [
            ActivityKindName::Message,
            ActivityKindName::Command,
            ActivityKindName::Interaction,
            ActivityKindName::Event
        ]
    );
}

#[test]
fn a_legacy_blob_has_no_routing_and_no_consumer_keys() {
    let mut value = routed();
    value.as_object_mut().unwrap().remove("routing");
    value["secrets"]
        .as_object_mut()
        .unwrap()
        .remove("consumer_keys");
    let legacy = definition(value);
    let blob = legacy.to_blob_json();
    assert!(blob.get("routing").is_none());
    assert!(blob["secrets"].get("consumer_keys").is_none());
}

#[test]
fn a_routed_blob_keeps_its_consumer_keys_and_round_trips() {
    let routed = definition(routed());
    let blob = routed.to_blob_json();
    assert_eq!(
        blob["secrets"]["consumer_keys"],
        json!([[REVIEWER, agent_key(REVIEWER)], [NYX, agent_key(NYX)]])
    );
    assert_eq!(
        blob["routing"],
        serde_json::to_value(&routed.routing).unwrap()
    );
    let restored: ChannelDefinition = serde_json::from_str(&canonical_json(blob.clone())).unwrap();
    assert_eq!(restored, routed);
    assert_eq!(restored.secrets.consumer_keys, routed.secrets.consumer_keys);
    assert_eq!(
        canonical_json(restored.to_blob_json()),
        canonical_json(blob)
    );
}

#[test]
fn neither_the_public_view_nor_debug_output_carries_a_key() {
    let routed = definition(routed());
    let public = routed.to_public_json().to_string();
    let debug = format!("{routed:?}");
    for consumer in ["default", REVIEWER, NYX] {
        assert!(!public.contains(&agent_key(consumer)), "{public}");
        assert!(!debug.contains(&agent_key(consumer)), "{debug}");
    }
    assert!(!public.contains("consumer_keys"));
    // The public view keeps every consumer's public key hash.
    assert!(public.contains(&super::fixture::hash(&agent_key(REVIEWER))));
}
