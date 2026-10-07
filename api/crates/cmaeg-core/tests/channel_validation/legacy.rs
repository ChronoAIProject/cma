//! Characterization of today's channel source and partition rules.
//!
//! These rows pin the legacy predicate: management answers a refused write
//! with exactly this pointer and message, and later readers decide legacy
//! compatibility against the same rules. A row records what the code returns
//! today; it changes only when the legacy rules are deliberately changed.
//!
//! Three refusals in that code are unreachable through a deserialized
//! definition and so have no row:
//! - `platform is duplicated`: a second relay of any platform is refused as a
//!   duplicated variant first.
//! - `requires a chat source` for `thread`: sources are never empty there, and
//!   a list without a chat source is custom-only, which trips the custom-key
//!   rule first.
//! - a native Telegram `bot_id` refused as a non-numeric user ID: the native
//!   check already demands a canonical positive 64-bit integer.

use cmaeg_core::channel::ChannelDefinition;
use serde_json::{json, Value};

use super::{channel, relay, relay_channel, relay_on, validate};

#[path = "legacy_refusals.rs"]
mod refusals;
use refusals::refusals;

fn edit(mut value: Value, change: impl FnOnce(&mut Value)) -> Value {
    change(&mut value);
    value
}

fn sources(base: Value, list: Value) -> Value {
    edit(base, |value| value["sources"] = list)
}

fn partition(base: Value, key: &str) -> Value {
    edit(base, |value| value["partition"]["key"] = json!(key))
}

fn custom() -> Value {
    channel()["sources"][0].clone()
}

fn telegram() -> Value {
    json!({"type":"telegram","bot_id":"77","bot_username":"fixture_bot","admission":{"type":"all"}})
}

fn lark_relay() -> Value {
    edit(relay(), |source| source["platform"] = json!("lark"))
}

fn telegram_channel() -> Value {
    edit(sources(relay_channel(), json!([telegram()])), |value| {
        value["secrets"]["telegram_bot_token"] = json!("77:fixture");
        value["secrets"]["telegram_webhook_secret"] = json!("native-webhook-fixture");
    })
}

fn telegram_source(field: &str, replacement: Value) -> Value {
    edit(telegram_channel(), |value| {
        value["sources"][0][field] = replacement;
    })
}

fn telegram_secret(field: &str, replacement: Value) -> Value {
    edit(telegram_channel(), |value| {
        value["secrets"][field] = replacement;
    })
}

fn relay_source(field: &str, replacement: Value) -> Value {
    edit(relay_channel(), |value| {
        value["sources"][0][field] = replacement;
    })
}

fn custom_source(field: &str, replacement: Value) -> Value {
    edit(channel(), |value| value["sources"][0][field] = replacement)
}

fn allowlist(ids: Value) -> Value {
    json!({"type":"allowlist","sender_ids":ids})
}

fn scoped(senders: Value, chats: Value) -> Value {
    json!({"type":"scoped","senders":senders,"chats":chats,"groups":"deny"})
}

fn listed(ids: Value) -> Value {
    json!({"type":"allowlist","ids":ids})
}

fn open() -> Value {
    json!({"type":"open"})
}

fn hashes(hashes: Value) -> Value {
    custom_source("auth", json!({"type":"nyxid_key","key_hashes":hashes}))
}

fn attachments(setting: Value) -> Value {
    custom_source("attachments", setting)
}

fn hash() -> String {
    format!("sha256:{}", "a".repeat(64))
}

/// One accepted fixture per legal shape of the legacy rules. No four-source
/// shape exists: three distinct variants is the most the rules admit.
fn valid_shapes() -> Vec<Value> {
    let lark_bot = json!({"bot_id":"ou_7d8a6e6df7621556ce0d21922b676706"});
    let normalized = |bot_id: &str| json!({"bot_id":bot_id,"trust_normalized":true});
    vec![
        telegram_channel(),
        partition(telegram_channel(), "conversation"),
        relay_on(
            "telegram",
            json!({"bot_id":"12345","bot_username":"support_bot"}),
        ),
        relay_on("lark", lark_bot.clone()),
        relay_on("feishu", lark_bot),
        relay_on("discord", normalized("B012ABCDEF")),
        relay_on("slack", normalized("B012ABCDEF")),
        relay_on("whatsapp", normalized("wa-bot.1")),
        partition(relay_channel(), "thread"),
        partition(channel(), "fixed"),
        partition(channel(), "event_subject"),
        partition(channel(), "per_event"),
        edit(hashes(json!([hash()])), |value| {
            value["sources"][0]["prompt_template"] = json!("x".repeat(8 * 1024));
            value["sources"][0]["attachments"] = json!({"origins":["https://cdn.example"]});
        }),
        partition(
            sources(
                telegram_channel(),
                json!([telegram(), lark_relay(), custom()]),
            ),
            "fixed",
        ),
    ]
}

#[test]
fn legacy_source_and_partition_refusals_are_pinned() {
    for (value, pointer, message) in refusals() {
        let shown = value.to_string();
        let error = validate(value).expect_err(&shown);
        assert_eq!(
            (error.pointer.as_str(), error.message.as_str()),
            (pointer, message),
            "{shown}"
        );
    }
}

#[test]
fn legacy_valid_shapes_are_accepted() {
    for value in valid_shapes() {
        let shown = value.to_string();
        validate(value).unwrap_or_else(|error| panic!("{shown}: {error}"));
    }
}

#[test]
fn legacy_source_and_partition_predicate_answers_only_for_its_own_rules() {
    let parse = |value| serde_json::from_value::<ChannelDefinition>(value).unwrap();
    for (value, pointer, message) in refusals() {
        // This row pins the head check ahead of the sources it also breaks.
        if pointer == "/profile_digest" {
            continue;
        }
        let error = parse(value)
            .validate_legacy_sources_and_partition()
            .unwrap_err();
        assert_eq!(
            (error.pointer.as_str(), error.message.as_str()),
            (pointer, message)
        );
    }
    for value in valid_shapes() {
        for (pointer, replacement) in [
            ("/schema_version", json!(2)),
            ("/reply/sinks", json!(vec![json!({"type":"sync"}); 4])),
        ] {
            let mut broken = value.clone();
            *broken.pointer_mut(pointer).unwrap() = replacement;
            let definition = parse(broken);
            definition.validate_legacy_sources_and_partition().unwrap();
            assert_eq!(definition.validate_legacy().unwrap_err().pointer, pointer);
        }
    }
}
