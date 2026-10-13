//! A cached summary stays small however large a valid routing policy is:
//! eight consumers and 32 rules whose every match list holds 64 entries of
//! 512 bytes. Charges include retained allocation capacities, not only JSON.
//! Sequential cache tests settle Moka maintenance; the production bound is
//! best effort and does not include Arcs retained by readers after eviction.
use std::{collections::BTreeSet, sync::Arc};

use cmaeg::console::{
    routing_view::PATTERN_TEXT_BYTES,
    summary::{ChannelSummary, Summaries, SummaryStatus, SUMMARY_UNIT_BYTES},
};
use cmaeg_core::channel::{ChannelDefinition, MAX_MATCH_ENTRIES, MAX_MATCH_ENTRY_BYTES, MAX_RULES};
use serde_json::{json, Value};

use super::{
    pages_support::{self, record},
    policy::{self, agent_key, NYX, REVIEWER},
    routed_support,
};

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

/// The bytes a summary serializes to, which its units must cover.
fn serialized(summary: &ChannelSummary) -> u64 {
    u64::try_from(serde_json::to_vec(summary).unwrap().len()).unwrap()
}

#[test]
fn a_summary_is_weighed_by_at_least_its_serialized_size() {
    let mut single = pages_support::definition(&record(1), "alice", "orders");
    single.sources.truncate(1);
    let summaries = [
        // Rich channels pay for every nested allocation, including buffers
        // whose String/Vec headers have no serialized representation.
        (summary_of(&maximal()), 80),
        (summary_of(&single), 4),
        (
            summary_of(&pages_support::definition(&record(1), "alice", "orders")),
            8,
        ),
        (
            summary_of(&routed_support::routed_with(
                &record(2),
                "alice",
                "routed",
                |_| {},
            )),
            8,
        ),
        (
            ChannelSummary::without_definition("alice", SummaryStatus::Unreadable),
            1,
        ),
    ];
    for (summary, most) in &summaries {
        let units = summary.units();
        let bytes = serialized(summary);
        assert!(
            bytes <= u64::from(units) * SUMMARY_UNIT_BYTES,
            "{bytes} bytes in {units} units"
        );
        assert!(
            units <= *most,
            "{bytes} bytes take {units} units, not at most {most}"
        );
    }
}

#[test]
fn the_summary_cache_holds_at_most_its_units() {
    const MAX_UNITS: u64 = 100;
    let maximal = Arc::new(summary_of(&maximal()));
    let units = u64::from(maximal.units());
    let cache = Summaries::new(MAX_UNITS);
    for index in 0..50 {
        cache.insert(record(index), 1, maximal.clone());
    }
    let held = (0..50)
        .filter(|index| cache.get(&record(*index), 1).is_some())
        .count();
    let held = u64::try_from(held).unwrap();
    assert!(held >= 1, "a maximal summary fits in {MAX_UNITS} units");
    assert!(
        held * units <= MAX_UNITS,
        "{held} summaries of {units} units"
    );
    // One-unit summaries fill the same cache one per unit.
    let small = Arc::new(ChannelSummary::without_definition(
        "alice",
        SummaryStatus::Unreadable,
    ));
    assert_eq!(small.units(), 1);
    let cache = Summaries::new(MAX_UNITS);
    let indexes = 0..usize::try_from(MAX_UNITS).unwrap();
    for index in indexes.clone() {
        cache.insert(record(index), 1, small.clone());
    }
    let held = indexes
        .filter(|index| cache.get(&record(*index), 1).is_some())
        .count();
    assert_eq!(u64::try_from(held).unwrap(), MAX_UNITS);
}

#[test]
fn nested_spare_capacity_is_charged_even_when_serialization_is_unchanged() {
    let mut summary = summary_of(&maximal());
    let before = serialized(&summary);
    let before_units = summary.units();
    // Reserve space without changing displayed content. A serialization-only
    // estimator cannot see either a nested vector buffer or a string buffer.
    let rules = &mut summary
        .detail
        .as_mut()
        .unwrap()
        .routing
        .as_mut()
        .unwrap()
        .rules;
    rules[0].matches.reserve_exact(4096);
    rules[0].matches[0].reserve_exact(65_536);
    let buffers = rules[0].matches.capacity() * std::mem::size_of::<String>()
        + rules[0].matches[0].capacity();
    assert_eq!(serialized(&summary), before);
    assert!(summary.units() > before_units + 64);
    assert!(u64::from(summary.units()) * SUMMARY_UNIT_BYTES >= buffers as u64);
}

#[test]
fn an_overweight_summary_is_not_cached_but_a_long_record_key_is_supported() {
    const MAX_UNITS: u64 = 100;
    let cache = Summaries::new(MAX_UNITS);
    let mut large = ChannelSummary::without_definition("alice", SummaryStatus::Unreadable);
    large.name.reserve_exact(256 * 1024);
    assert!(u64::from(large.units()) > MAX_UNITS);
    cache.insert(record(900), 1, Arc::new(large));
    assert!(cache.get(&record(900), 1).is_none());

    // RecordId keeps input String capacity. Insertion re-copies its validated
    // bytes and charges the full 512-byte key bound separately from metadata.
    let mut text = String::with_capacity(256 * 1024);
    text.push_str(&"x".repeat(cmaeg_core::ids::MAX_ID_BYTES));
    let id = cmaeg_core::ids::RecordId::new(text).unwrap();
    let summary = Arc::new(ChannelSummary::without_definition(
        "alice",
        SummaryStatus::Unreadable,
    ));
    let lookup = id.clone();
    cache.insert(id, 2, summary.clone());
    assert_eq!(cache.get(&lookup, 2).unwrap(), summary);
}

#[test]
fn heterogeneous_summaries_share_the_charged_budget() {
    const MAX_UNITS: u64 = 100;
    let cache = Summaries::new(MAX_UNITS);
    for index in 0..80 {
        let mut summary = ChannelSummary::without_definition("alice", SummaryStatus::Unreadable);
        summary.name.reserve_exact((index % 7) * 2048);
        cache.insert(record(index), 1, Arc::new(summary));
    }
    let retained: Vec<_> = (0..80)
        .filter_map(|index| cache.get(&record(index), 1))
        .collect();
    let charged: u64 = retained
        .iter()
        .map(|summary| u64::from(summary.units()))
        .sum();
    assert!(!retained.is_empty());
    assert!(charged <= MAX_UNITS, "retained {charged} units");
}
