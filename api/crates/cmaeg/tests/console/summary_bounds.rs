//! A cached summary stays small however large a valid routing policy is:
//! eight consumers and 32 rules whose every match list holds 64 entries of
//! 512 bytes.
use std::collections::BTreeSet;

use cmaeg::console::{routing_view::PATTERN_TEXT_BYTES, summary::ChannelSummary};
use cmaeg_core::channel::{ChannelDefinition, MAX_MATCH_ENTRIES, MAX_MATCH_ENTRY_BYTES, MAX_RULES};
use serde_json::{json, Value};

use super::policy::{self, agent_key, NYX, REVIEWER};

/// Every source slot a rule may name; repeated to fill a 64-entry list.
const SOURCES: [&str; 8] = [
    "telegram",
    "custom",
    "relay_telegram",
    "relay_lark",
    "relay_feishu",
    "relay_discord",
    "relay_slack",
    "relay_whatsapp",
];
const KINDS: [&str; 4] = ["message", "command", "interaction", "event"];

/// `count` distinct entries of exactly `bytes` bytes, the last character
/// `tail` so a `*` makes each a prefix pattern.
fn entries(prefix: &str, count: usize, bytes: usize, tail: char) -> Vec<String> {
    (0..count)
        .map(|index| {
            let head = format!("{prefix}{index:02}");
            format!("{head:x<width$}{tail}", width = bytes - 1)
        })
        .collect()
}

/// The routed fixture at every bound the policy validation allows: eight
/// consumers with the longest names and labels, and [`MAX_RULES`] rules of
/// full match lists. The first half's patterns are each 512 bytes; the
/// second half's are short enough that some are shown.
pub fn maximal() -> ChannelDefinition {
    let mut value = policy::routed();
    let mut consumers = vec![
        value["routing"]["consumers"][0].clone(),
        value["routing"]["consumers"][1].clone(),
    ];
    let mut ids = vec!["default".to_owned(), REVIEWER.to_owned(), NYX.to_owned()];
    let mut keys = vec![
        json!([REVIEWER, agent_key(REVIEWER)]),
        json!([NYX, agent_key(NYX)]),
    ];
    for index in 0..5 {
        let id = format!("csm_{:012x}", 0x10 + index);
        let name = format!("consumer-{index:0>23}");
        consumers.push(policy::consumer(&id, &name, "cma_codex"));
        keys.push(json!([id, agent_key(&id)]));
        ids.push(id);
    }
    for consumer in &mut consumers {
        consumer["label"] = "Ü".repeat(64).into();
    }
    let routing = &mut value["routing"];
    routing["consumers"] = consumers.into();
    routing["default_consumer"]["label"] = "Ü".repeat(64).into();
    routing["max_targets"] = 8.into();
    routing["default_targets"] = json!(ids);
    let rules: Vec<Value> = (0..MAX_RULES)
        .map(|index| {
            let pattern_bytes = if index < MAX_RULES / 2 {
                MAX_MATCH_ENTRY_BYTES
            } else {
                40
            };
            let full = |prefix: &str, tail: char| {
                entries(prefix, MAX_MATCH_ENTRIES, MAX_MATCH_ENTRY_BYTES, tail)
            };
            json!({
                "id": format!("rule-{index:0>27}"),
                "when": {
                    "sources": SOURCES.iter().cycle().take(MAX_MATCH_ENTRIES).collect::<Vec<_>>(),
                    "kinds": KINDS.iter().cycle().take(MAX_MATCH_ENTRIES).collect::<Vec<_>>(),
                    "chats": full("chat-", 'x'),
                    "senders": full("sender-", 'x'),
                    "event_types": entries("com.example.", MAX_MATCH_ENTRIES, pattern_bytes, '*'),
                    "event_sources": entries("https://ci.example/", MAX_MATCH_ENTRIES, pattern_bytes, '*'),
                },
                "targets": ids,
            })
        })
        .collect();
    routing["rules"] = rules.into();
    value["secrets"]["consumer_keys"] = keys.into();
    let definition = policy::definition(value);
    definition.validate().unwrap();
    definition
}

pub fn summary_of(definition: &ChannelDefinition) -> ChannelSummary {
    let providers = BTreeSet::from(["cma_codex".to_owned()]);
    ChannelSummary::of(definition, 65_536, true, &providers)
}

#[test]
fn every_rule_of_a_maximal_policy_projects_to_a_bounded_size() {
    let summary = summary_of(&maximal());
    let routing = summary.detail.as_ref().unwrap().routing.as_ref().unwrap();
    assert_eq!(routing.rules.len(), MAX_RULES);
    for (index, rule) in routing.rules.iter().enumerate() {
        let sources: Vec<&str> = rule.sources.iter().map(|source| source.as_str()).collect();
        assert_eq!(sources, SOURCES, "each source named once, in order");
        assert_eq!(
            rule.matches[0],
            "kinds message, command, interaction, event"
        );
        assert_eq!(rule.matches[1], "64 chat(s)");
        assert_eq!(rule.matches[2], "64 sender(s)");
        if index < MAX_RULES / 2 {
            assert_eq!(rule.matches[3], "64 event type(s)");
            assert_eq!(rule.matches[4], "64 event source(s)");
        } else {
            for (list, noun) in rule.matches[3..]
                .iter()
                .zip(["event types ", "event sources "])
            {
                let shown = list.strip_prefix(noun).unwrap();
                let (patterns, more) = shown.split_once(" +").unwrap();
                assert!(patterns.len() <= PATTERN_TEXT_BYTES, "{list}");
                assert!(patterns.ends_with('*'), "whole patterns only: {list}");
                assert!(more.ends_with(" more"), "{list}");
            }
        }
        let bytes = serde_json::to_vec(rule).unwrap().len();
        assert!(bytes <= 1024, "rule {index} projects to {bytes} bytes");
    }
}
