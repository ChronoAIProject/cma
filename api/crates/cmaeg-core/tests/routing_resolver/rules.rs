//! Rule matching: AND across lists, OR within a list, event-only lists,
//! patterns, inert rules and first-match order, each recorded in the trace.
use cmaeg_core::{
    activity::Activity,
    channel::{ChannelDefinition, RoutingRule},
    routing::{
        rules::{first_match, matches},
        TraceStep,
    },
};
use serde_json::{json, Value};

use super::fixture::{
    channel, click, command, custom, event, lark, message, rule, telegram, trace, unrouted,
};

fn parsed(when: Value) -> RoutingRule {
    serde_json::from_value(rule("r", when, json!(["default"]))).unwrap()
}

fn outcome(when: Value, activity: &Activity) -> (bool, Option<&'static str>) {
    matches(&parsed(when), activity, &activity.source.source_id())
}

fn with_rules(rules: Value) -> ChannelDefinition {
    channel(|value| value["routing"]["rules"] = rules)
}

#[test]
fn an_empty_match_admits_every_kind_and_source() {
    for activity in [
        message(telegram(), "hi"),
        command(lark(), "status", ""),
        click(telegram()),
        event("build.done", json!({})),
    ] {
        assert_eq!(outcome(json!({}), &activity), (true, None));
    }
}

#[test]
fn lists_are_anded_and_entries_are_ored() {
    let when = json!({"sources": ["telegram", "relay_lark"], "kinds": ["message"],
        "chats": ["chat-0", "chat-1"], "senders": ["user-1"]});
    assert_eq!(
        outcome(when.clone(), &message(telegram(), "x")),
        (true, None)
    );
    assert_eq!(outcome(when.clone(), &message(lark(), "x")), (true, None));
    let custom_message = message(custom(), "x");
    assert_eq!(
        outcome(when.clone(), &custom_message),
        (false, Some("sources"))
    );
    let status = command(telegram(), "status", "");
    assert_eq!(outcome(when.clone(), &status), (false, Some("kinds")));
    let mut elsewhere = message(telegram(), "x");
    elsewhere.conversation.id = "chat-10".into();
    assert_eq!(outcome(when.clone(), &elsewhere), (false, Some("chats")));
    let mut stranger = message(telegram(), "x");
    stranger.actor.id = "user-10".into();
    assert_eq!(outcome(when, &stranger), (false, Some("senders")));
}

#[test]
fn kinds_name_each_activity_variant() {
    for (kind, activity) in [
        ("message", message(telegram(), "x")),
        ("command", command(telegram(), "status", "")),
        ("interaction", click(telegram())),
        ("event", event("build.done", json!({}))),
    ] {
        assert_eq!(outcome(json!({"kinds": [kind]}), &activity), (true, None));
        let others: Vec<&str> = ["message", "command", "interaction", "event"]
            .into_iter()
            .filter(|other| *other != kind)
            .collect();
        let refused = outcome(json!({"kinds": others}), &activity);
        assert_eq!(refused, (false, Some("kinds")), "{kind}");
    }
}

#[test]
fn event_patterns_match_exactly_or_by_trailing_prefix() {
    let done = event("build.done", json!({}));
    for (pattern, matched) in [
        ("build.done", true),
        ("build.*", true),
        ("*", true),
        ("build", false),
        ("build.done.*", false),
        ("Build.done", false),
    ] {
        let when = json!({"event_types": [pattern]});
        let expected = (matched, (!matched).then_some("event_types"));
        assert_eq!(outcome(when, &done), expected, "{pattern}");
    }
    let sources = json!({"event_sources": ["https://ci.example/*"]});
    assert_eq!(outcome(sources, &done), (true, None));
    let other = json!({"event_sources": ["https://cd.example/*"]});
    assert_eq!(outcome(other, &done), (false, Some("event_sources")));
}

#[test]
fn event_lists_never_match_other_kinds() {
    for activity in [message(telegram(), "build.done"), click(telegram())] {
        let types = json!({"event_types": ["*"]});
        assert_eq!(outcome(types, &activity), (false, Some("event_types")));
        let sources = json!({"event_sources": ["*"]});
        assert_eq!(outcome(sources, &activity), (false, Some("event_sources")));
    }
}

#[test]
fn the_first_matching_active_rule_wins_and_every_rule_tried_is_recorded() {
    let def = with_rules(json!([
        rule(
            "slack",
            json!({"sources": ["relay_slack"]}),
            json!(["default"])
        ),
        rule(
            "lark",
            json!({"sources": ["relay_lark"]}),
            json!(["default"])
        ),
        rule("first", json!({"kinds": ["message"]}), json!(["default"])),
        rule("second", json!({}), json!(["default"])),
    ]));
    let mut steps = trace();
    let activity = message(telegram(), "hello");
    let matched = first_match(&def, &activity, &mut steps).unwrap();
    assert_eq!(matched.id.as_str(), "first");
    let tried = |id: &str, matched, inactive, failed_on| TraceStep::RuleTried {
        rule_id: serde_json::from_value(json!(id)).unwrap(),
        matched,
        inactive,
        failed_on,
    };
    assert_eq!(
        steps.steps(),
        [
            tried("slack", false, true, None),
            tried("lark", false, false, Some("sources")),
            tried("first", true, false, None),
        ]
    );
    assert!(!steps.truncated());
}

#[test]
fn an_inert_rule_never_matches_even_when_its_other_lists_would() {
    // relay_slack is not attached, so the rule is inert though `kinds` fits.
    let def = with_rules(json!([rule(
        "slack",
        json!({"sources": ["relay_slack"], "kinds": ["message"]}),
        json!(["default"])
    )]));
    let mut steps = trace();
    assert!(first_match(&def, &message(telegram(), "x"), &mut steps).is_none());
    assert!(matches!(
        steps.steps(),
        [TraceStep::RuleTried {
            inactive: true,
            matched: false,
            failed_on: None,
            ..
        }]
    ));
}

#[test]
fn no_rule_matches_without_rules_or_without_routing() {
    let mut steps = trace();
    let activity = message(telegram(), "x");
    assert!(first_match(&with_rules(json!([])), &activity, &mut steps).is_none());
    assert!(first_match(&unrouted(), &activity, &mut steps).is_none());
    assert!(steps.steps().is_empty());
}

#[test]
fn rule_ids_are_the_only_rule_data_a_trace_keeps() {
    let def = with_rules(json!([rule(
        "chats",
        json!({"chats": ["chat-secret-room"]}),
        json!(["default"])
    )]));
    let mut steps = trace();
    let _ = first_match(&def, &message(telegram(), "private words"), &mut steps);
    let shown = format!("{steps:?}");
    assert!(!shown.contains("chat-secret-room") && !shown.contains("private words"));
}

#[test]
fn thirty_two_failing_rules_fill_sixteen_steps_and_mark_the_trace_truncated() {
    let rules: Vec<Value> = (0..32)
        .map(|index| {
            rule(
                &format!("r{index}"),
                json!({"chats": ["elsewhere"]}),
                json!(["default"]),
            )
        })
        .collect();
    let def = with_rules(Value::Array(rules));
    let mut steps = trace();
    assert!(first_match(&def, &message(telegram(), "x"), &mut steps).is_none());
    assert_eq!(steps.steps().len(), cmaeg_core::routing::MAX_TRACE_STEPS);
    assert!(steps.truncated());
}
