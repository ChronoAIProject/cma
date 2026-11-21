//! The reader's extended source and partition rules. Each refusal pins its
//! exact pointer and message; the legacy rules keep refusing every one of
//! these shapes, which only the write gate's flags will admit.

use cmaeg_core::{
    activity::SourceKind,
    channel::{ChannelDefinition, PartitionKey, PartitionRule, Platform},
    ValidationError,
};
use serde_json::{json, Value};

use super::{channel, relay, relay_channel};

const PLATFORMS: [&str; 6] = ["telegram", "lark", "feishu", "discord", "slack", "whatsapp"];
const KEYS: [&str; 6] = [
    "conversation_and_sender",
    "conversation",
    "thread",
    "fixed",
    "event_subject",
    "per_event",
];

fn parse(value: Value) -> ChannelDefinition {
    serde_json::from_value(value).unwrap()
}

fn reader(value: Value) -> Result<(), ValidationError> {
    parse(value).validate()
}

fn refusal(value: Value) -> (String, String) {
    let error = reader(value).unwrap_err();
    (error.pointer, error.message)
}

fn expect(value: Value, pointer: &str, message: &str) {
    let shown = value.to_string();
    assert_eq!(
        refusal(value),
        (pointer.to_owned(), message.to_owned()),
        "{shown}"
    );
}

/// A relay on `platform` admitting `route`, legal on its own.
fn relay_on(platform: &str, route: &str) -> Value {
    let mut source = relay();
    source["platform"] = json!(platform);
    source["route_ids"] = json!([route]);
    if matches!(platform, "discord" | "slack" | "whatsapp") {
        source["trust_normalized"] = json!(true);
    }
    source
}

fn telegram() -> Value {
    json!({"type":"telegram","bot_id":"77","bot_username":"fixture_bot","admission":{"type":"all"}})
}

fn custom() -> Value {
    channel()["sources"][0].clone()
}

/// A channel with these sources under `key`, with every credential they need.
fn with(sources: Value, key: &str) -> Value {
    let mut value = relay_channel();
    value["sources"] = sources;
    value["partition"]["key"] = json!(key);
    value["secrets"]["telegram_bot_token"] = json!("77:fixture");
    value["secrets"]["telegram_webhook_secret"] = json!("native-webhook-fixture");
    value
}

fn custom_key(mut value: Value, key: &str) -> Value {
    value["partition"]["custom_key"] = json!(key);
    value
}

fn two_relays() -> Value {
    json!([relay_on("telegram", "route-1"), relay_on("lark", "route-2")])
}

#[test]
fn readers_accept_relays_on_distinct_platforms_with_a_shared_key() {
    let value = with(two_relays(), "conversation_and_sender");
    let definition = parse(value);
    definition.validate().unwrap();
    // The legacy rules, which writes still answer with, refuse it.
    let error = definition.validate_legacy().unwrap_err();
    assert_eq!(
        (error.pointer.as_str(), error.message.as_str()),
        ("/sources/1", "source variant is duplicated")
    );
}

#[test]
fn readers_refuse_more_than_eight_sources() {
    let mut sources = vec![telegram(), custom()];
    sources.extend(
        PLATFORMS
            .iter()
            .enumerate()
            .map(|(index, platform)| relay_on(platform, &format!("route-{index}"))),
    );
    sources.push(relay_on("lark", "route-extra"));
    expect(
        with(json!(sources), "conversation"),
        "/sources",
        "must contain at most eight sources",
    );
}

#[test]
fn readers_refuse_duplicated_variants_and_platforms() {
    let duplicated = "source variant is duplicated";
    for (sources, pointer, message) in [
        (
            json!([
                relay_on("telegram", "route-1"),
                relay_on("telegram", "route-2")
            ]),
            "/sources/1",
            "platform is duplicated",
        ),
        (
            json!([relay_on("lark", "route-1"), telegram(), telegram()]),
            "/sources/2",
            duplicated,
        ),
        (
            json!([
                relay_on("lark", "route-1"),
                relay_on("feishu", "route-2"),
                custom(),
                custom()
            ]),
            "/sources/3",
            duplicated,
        ),
    ] {
        expect(
            custom_key(with(sources, "conversation"), "fixed"),
            pointer,
            message,
        );
    }
}

#[test]
fn relays_must_share_their_signing_identity() {
    for (field, other) in [
        ("key_id", json!("other-key")),
        ("issuer", json!("https://other.example")),
        ("jwks_url", json!("https://identity.example/other-jwks")),
    ] {
        let mut sources = two_relays();
        sources[1][field] = other;
        expect(
            with(sources, "conversation"),
            "/sources/1/key_id",
            "relay sources must share issuer, jwks_url and key_id",
        );
    }
}

#[test]
fn relay_routes_must_be_disjoint() {
    let mut sources = two_relays();
    sources[1]["route_ids"] = json!(["route-9", "route-1"]);
    expect(
        with(sources, "conversation"),
        "/sources/1/route_ids",
        "route id is already used by another relay source",
    );
}

#[test]
fn every_source_keeps_its_own_rules() {
    let mut sources = two_relays();
    sources[1]["jwks_url"] = json!("http://identity.example/jwks");
    expect(
        with(sources, "conversation"),
        "/sources/1/jwks_url",
        "must use https (or local http)",
    );
}

#[test]
fn every_present_source_needs_an_applicable_key() {
    let custom_message = "custom sources require fixed, event_subject, or per_event";
    let mut sources = two_relays();
    sources.as_array_mut().unwrap().push(custom());
    expect(
        with(sources.clone(), "conversation"),
        "/partition/key",
        custom_message,
    );
    expect(
        with(
            json!([relay_on("lark", "route-1"), custom()]),
            "conversation",
        ),
        "/partition/key",
        custom_message,
    );
    expect(
        with(two_relays(), "per_event"),
        "/partition/key",
        "chat sources cannot use per_event",
    );
    // A custom key that suits the custom source makes the same list valid.
    reader(custom_key(with(sources, "conversation"), "event_subject")).unwrap();
}

#[test]
fn custom_key_needs_a_chat_key_and_must_itself_be_a_custom_key() {
    for key in ["fixed", "event_subject", "per_event"] {
        expect(
            custom_key(with(json!([custom()]), key), "fixed"),
            "/partition/custom_key",
            "requires a chat partition key",
        );
    }
    for other in ["conversation", "conversation_and_sender", "thread"] {
        expect(
            custom_key(
                with(json!([relay_on("lark", "route-1")]), "conversation"),
                other,
            ),
            "/partition/custom_key",
            "must be fixed, event_subject, or per_event",
        );
    }
    let mut value = custom_key(with(two_relays(), "conversation"), "fixed");
    value["partition"]["version"] = json!(2);
    expect(value, "/partition/version", "unsupported version");
}

#[test]
fn a_channel_may_have_no_sources() {
    let mut value = with(json!([]), "thread");
    value["reply"]["sinks"] = json!([{"type":"sync"}]);
    reader(value.clone()).unwrap();
    // A reply to the source still needs a source.
    value["reply"]["sinks"] = json!([{"type":"source"}]);
    expect(value, "/reply/sinks", "source sink needs a source");
}

#[test]
fn a_channel_may_hold_eight_sources() {
    let mut sources = vec![telegram()];
    sources.extend(
        PLATFORMS
            .iter()
            .enumerate()
            .map(|(index, platform)| relay_on(platform, &format!("route-{index}"))),
    );
    sources.push(custom());
    let value = custom_key(with(json!(sources), "conversation"), "event_subject");
    assert_eq!(value["sources"].as_array().unwrap().len(), 8);
    reader(value).unwrap();
}

#[test]
fn keys_stay_inert_without_a_source_of_their_kind() {
    // `thread` without any chat source.
    reader(custom_key(with(json!([custom()]), "thread"), "fixed")).unwrap();
    // `custom_key` without a custom source, on an otherwise legacy channel.
    let value = custom_key(
        with(json!([relay_on("lark", "route-1")]), "conversation"),
        "per_event",
    );
    let definition = parse(value);
    definition.validate_legacy().unwrap();
    definition.validate().unwrap();
}

#[test]
fn the_custom_key_applies_only_to_custom_activities() {
    let rule = PartitionRule {
        version: 1,
        key: PartitionKey::Conversation,
        custom_key: Some(PartitionKey::EventSubject),
    };
    let relay = SourceKind::NyxidRelay {
        platform: Platform::Lark,
        route_id: "route-1".into(),
    };
    assert_eq!(
        rule.key_for(&SourceKind::Custom),
        PartitionKey::EventSubject
    );
    assert_eq!(rule.key_for(&relay), PartitionKey::Conversation);
    let legacy = PartitionRule {
        custom_key: None,
        ..rule
    };
    assert_eq!(
        legacy.key_for(&SourceKind::Custom),
        PartitionKey::Conversation
    );
}

/// Every subset of {telegram, one relay per platform, custom} of one to
/// four sources, under every key, with and without sinks.
fn legacy_fixture_space() -> Vec<Value> {
    let mut members = vec![telegram(), custom()];
    members.extend(
        PLATFORMS
            .iter()
            .map(|platform| relay_on(platform, "route-1")),
    );
    let mut space = Vec::new();
    for mask in 1_u32..(1 << members.len()) {
        if !(1..=4).contains(&mask.count_ones()) {
            continue;
        }
        let sources: Vec<_> = (0..members.len())
            .filter(|bit| mask & (1 << bit) != 0)
            .map(|bit| members[bit].clone())
            .collect();
        for key in KEYS {
            for sinks in [
                json!([]),
                json!([{"type":"source"},{"type":"sync"},
                    {"type":"result_webhook","url":"https://callback.example","hmac_key_id":"key"}]),
            ] {
                let mut value = with(json!(sources), key);
                value["reply"]["sinks"] = sinks;
                space.push(value);
            }
        }
    }
    space
}

#[test]
fn extended_readers_accept_every_legacy_valid_definition() {
    let (mut legacy_valid, mut legacy_invalid) = (0, 0);
    for value in legacy_fixture_space() {
        let shown = value.to_string();
        let definition = parse(value);
        if definition.validate_legacy().is_ok() {
            legacy_valid += 1;
            definition
                .validate()
                .unwrap_or_else(|error| panic!("{shown}: {error}"));
        } else {
            legacy_invalid += 1;
        }
    }
    // 162 source subsets, six keys, two sink lists; both outcomes occur.
    assert_eq!(legacy_valid + legacy_invalid, 162 * 6 * 2);
    assert!(legacy_valid > 0 && legacy_invalid > 0);
}
