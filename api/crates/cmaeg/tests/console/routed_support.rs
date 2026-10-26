//! A routed channel for the console page tests: the core's routing fixture
//! with rules (one of them inert), a pending consumer, publish policies and
//! a default reply source, under a console fixture owner and record.
use cmaeg_core::{
    channel::{ChannelCreation, ChannelDefinition},
    ids::RecordId,
};
use serde_json::{json, Value};

use super::{
    pages_support::{channel_id, record, World},
    policy::{self, agent_key, hash, NYX, REVIEWER, ROUTE},
};

/// A chat id one rule matches; rules show chats only by count.
pub const RULE_CHAT: &str = "oc_rule_chat_fixture_1";
/// A rule's `ce.source` pattern with userinfo and a query, which the
/// console strips like any `ce.source`.
pub const RULE_EVENT_SOURCE: &str = "https://ci-user:ci-pass@ci.example/build?token=fixture";

/// Every secret and full key hash the routed fixture holds; none may appear
/// on any page.
pub fn secrets() -> Vec<String> {
    let mut secrets: Vec<String> = ["default", REVIEWER, NYX]
        .into_iter()
        .flat_map(|consumer| [agent_key(consumer), hash(&agent_key(consumer))])
        .collect();
    secrets.extend(
        [
            "fixture-channel-key",
            "77:fixture",
            "fixture-webhook",
            "Zml4dHVyZQ==",
            RULE_CHAT,
            "ci-pass",
            "token=fixture",
        ]
        .map(str::to_owned),
    );
    secrets
}

/// The routed policy every routed page test reads: `default` presented as
/// `lead`, `reviewer` ready and publishing to `default`, `nyx` pending and
/// not mentionable, three rules of which `slack-only` names no attached
/// source, and the Lark relay's route as the default reply destination.
pub fn routed_value() -> Value {
    let mut value = policy::routed();
    let routing = &mut value["routing"];
    routing["default_consumer"] = json!({"name": "lead", "label": "Lead",
        "publish": {"enabled": true, "targets": []}});
    routing["consumers"][0]["publish"] = json!({"enabled": true, "targets": ["default"]});
    routing["consumers"][1]["pending"] = true.into();
    routing["consumers"][1]["mentionable"] = false.into();
    routing["consumers"][1]["non_human"] = true.into();
    routing["rules"] = json!([
        {"id": "ops-chat", "when": {"sources": ["relay_lark"], "kinds": ["message"],
            "chats": [RULE_CHAT]}, "targets": [REVIEWER, "default"]},
        {"id": "slack-only", "when": {"sources": ["relay_slack"]}, "targets": ["default"]},
        {"id": "ci", "when": {"event_types": ["com.example.ci.*"],
            "event_sources": [RULE_EVENT_SOURCE]}, "targets": [REVIEWER]}
    ]);
    routing["max_targets"] = 3.into();
    routing["reply_label"] = "always".into();
    routing["default_reply"] =
        json!({"type": "relay_conversation", "source_id": "relay_lark", "route_id": ROUTE});
    routing["agent_events"] =
        json!({"max_hops": 3, "per_consumer_per_minute": 5, "per_channel_per_minute": 20});
    value
}

/// [`routed_value`] with `edit` applied, at `record`, owned by `owner` and
/// named `name`, with a retained creation channel id. It must be a valid
/// stored definition.
pub fn routed_with(
    record: &RecordId,
    owner: &str,
    name: &str,
    edit: impl FnOnce(&mut Value),
) -> ChannelDefinition {
    let mut value = routed_value();
    edit(&mut value);
    value["record_id"] = record.as_str().into();
    value["owner"]["subject"] = owner.into();
    value["profile"]["metadata"]["owner"]["subject"] = owner.into();
    value["profile_digest"] = policy::digest_of(&value["profile"]).into();
    value["name"] = name.into();
    let mut definition: ChannelDefinition = serde_json::from_value(value).unwrap();
    definition.validate().unwrap();
    definition.creation = Some(ChannelCreation {
        channel_id: channel_id(record),
        fingerprint: "fixture".into(),
        ready: true,
    });
    definition
}

/// The routed channel at record `index`, stored in `world` at version 1.
pub fn add(world: &World, index: usize, owner: &str, name: &str) -> RecordId {
    let record = record(index);
    world.store(routed_with(&record, owner, name, |_| {}), 1);
    record
}
