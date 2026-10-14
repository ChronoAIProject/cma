//! I4: the provider conversation ids and idempotency keys of the default
//! consumer, pinned for every partition kind.
//!
//! A provider keys its conversations and turn admission by these values, so a
//! change to either derivation would silently start new conversations, or
//! admit a retried event twice, on every existing channel. The default
//! consumer keeps `ConversationId::derive` over the raw partition key and the
//! `evt_` idempotency key forever.
//!
//! The fixtures were generated on the parent commit of the change that added
//! per-consumer identities, with a fixed key ring and channel id. Updating one
//! is a reviewed contract change, never a way to make a test pass (see
//! `fixture.rs`).
use super::fixture::assert_golden;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use cmaeg_core::{
    activity::Activity,
    channel::PartitionRule,
    ids::{ChannelId, ConversationId, EventId, IdempotencyKey},
    seal::MasterKeyRing,
};
use serde_json::json;

fn keys() -> MasterKeyRing {
    MasterKeyRing::parse(&format!("1:{}", STANDARD.encode([4; 32]))).unwrap()
}

fn rule(key: &str) -> PartitionRule {
    serde_json::from_value(json!({"version": 1, "key": key})).unwrap()
}

fn chat() -> Activity {
    serde_json::from_value(json!({
        "schema_version": 1,
        "event_id": "message-1",
        "kind": {"type": "message", "text": "hello", "attachments": [], "mentions_bot": false},
        "actor": {"id": "7", "kind": "human"},
        "conversation": {"id": "42", "kind": "group", "thread_id": "9"},
        "occurred_at": "2026-09-22T00:00:00Z",
        "source": {"type": "nyxid_relay", "platform": "telegram", "route_id": "route-1"}
    }))
    .unwrap()
}

fn custom(subject: &str) -> Activity {
    serde_json::from_value(json!({
        "schema_version": 1,
        "event_id": "event-1",
        "kind": {"type": "event", "ce": {"specversion": "1.0", "id": "event-1", "source": "urn:golden:source", "type": "com.example.golden", "subject": subject}},
        "actor": {"id": "urn:golden:source", "kind": "system"},
        "conversation": {"id": subject, "kind": "custom"},
        "occurred_at": "2026-09-22T00:00:00Z",
        "source": {"type": "custom"}
    }))
    .unwrap()
}

#[test]
fn default_conversation_ids_are_unchanged_for_every_partition_kind() {
    let ring = keys();
    let channel = ChannelId::new("cmaeg1.ch.golden-identities").unwrap();
    let long_subject = format!("urn:golden:{}", "s".repeat(589));
    assert_eq!(long_subject.len(), 600);
    let cases = [
        ("conversation_and_sender", chat(), "conversation_and_sender"),
        ("conversation", chat(), "conversation"),
        ("thread", chat(), "thread"),
        ("fixed", custom("order-9"), "fixed"),
        ("event_subject", custom("order-9"), "event_subject"),
        (
            "event_subject_600_bytes",
            custom(&long_subject),
            "event_subject",
        ),
        ("per_event", custom("order-9"), "per_event"),
    ];
    let mut lines = String::new();
    for (name, activity, key) in cases {
        activity.validate().unwrap();
        let partition = activity.partition_key(&rule(key));
        let id = ConversationId::derive(&ring, &channel, 1, &partition);
        assert!(id.is_wellformed(), "{name}");
        lines.push_str(&format!("{name} = {id}\n"));
    }
    assert_golden("identities/conversations.txt", &lines);
}

#[test]
fn default_idempotency_keys_are_unchanged() {
    let cases = [
        ("ascii", "telegram:123456:1001".to_owned()),
        ("unsafe_characters", "lark/event 1:é?#".to_owned()),
        ("300_bytes", "e".repeat(300)),
    ];
    let mut lines = String::new();
    for (name, event_id) in cases {
        let key = IdempotencyKey::from_event(&EventId::new(event_id).unwrap());
        IdempotencyKey::new(key.as_str()).unwrap();
        lines.push_str(&format!("{name} = {}\n", key.as_str()));
    }
    assert_golden("identities/idempotency_keys.txt", &lines);
}
