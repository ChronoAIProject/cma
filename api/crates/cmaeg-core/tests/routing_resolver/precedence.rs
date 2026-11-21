//! The fixed precedence: click > hand-off > addressing > rules > default,
//! and the routing each step binds into the activity.
use cmaeg_core::{
    activity::{Activity, RouteResolution},
    channel::ChannelDefinition,
    ids::ConsumerId,
    routing::{explain, resolve, RouteError, TraceStep},
};
use serde_json::{json, Value};

use super::{
    fixture::{
        causation, channel, click, custom, decision, event, handoff, input, lark, message,
        routed_to, rule, telegram, unrouted,
    },
    policy::{NYX, REVIEWER},
};

fn ids(targets: &[&str]) -> Vec<String> {
    targets.iter().map(|id| (*id).to_owned()).collect()
}

/// A channel whose rule sends every message to `reviewer`.
fn with_message_rule() -> ChannelDefinition {
    channel(|value| {
        value["routing"]["rules"] = json!([rule(
            "chat",
            json!({"kinds": ["message"]}),
            json!([REVIEWER])
        )]);
    })
}

fn id(value: &str) -> ConsumerId {
    serde_json::from_value(json!(value)).unwrap()
}

/// Bind the decision into `activity` and validate it, as ingress will.
fn bound(def: &ChannelDefinition, activity: &Activity, input_card: Option<Option<&ConsumerId>>) {
    let mut route = input(def, activity);
    route.card_consumer = input_card;
    let routed = decision(def, &route);
    println!("{:?}: {:?}", routed.routing.resolution, routed.trace);
    let mut activity = activity.clone();
    activity.routing = Some(routed.routing);
    activity.validate().unwrap();
}

#[test]
fn a_click_outranks_every_other_step() {
    let def = with_message_rule();
    let activity = message(telegram(), "@nyx hello");
    let tokens = ids(&["nyx"]);
    let reviewer = id(REVIEWER);
    let mut route = input(&def, &activity);
    route.card_consumer = Some(Some(&reviewer));
    route.published = Some((&tokens, causation()));
    let outcome = resolve(&def, &route, true);
    assert_eq!(
        routed_to(outcome),
        Ok(Some((RouteResolution::Interaction, ids(&[REVIEWER]))))
    );
    // A default card goes to default even where a rule names reviewer.
    route.card_consumer = Some(None);
    let outcome = resolve(&def, &route, true);
    assert_eq!(
        routed_to(outcome),
        Ok(Some((RouteResolution::Interaction, ids(&["default"]))))
    );
    assert_eq!(
        decision(&def, &route).trace.steps(),
        [TraceStep::Precedence("interaction")]
    );
}

#[test]
fn a_hand_off_outranks_addressing_rules_and_defaults() {
    let def = with_message_rule();
    let activity = handoff("@nyx please summarize");
    let tokens = ids(&["reviewer", "main"]);
    let mut route = input(&def, &activity);
    route.published = Some((&tokens, causation()));
    let routed = decision(&def, &route);
    println!("agent_event: {:?}", routed.trace);
    let routing = routed.routing;
    assert_eq!(routing.resolution, RouteResolution::AgentEvent);
    assert_eq!(routing.source_id.as_str(), "agent");
    assert_eq!(routing.causation, Some(causation()));
    let targets: Vec<String> = routing.targets.iter().map(|t| t.id.to_string()).collect();
    assert_eq!(targets, ids(&[REVIEWER, "default"]));
    assert_eq!(routing.targets[1].name.as_str(), "main");
}

#[test]
fn addressing_outranks_rules_and_defaults() {
    let def = with_message_rule();
    for (text, resolution) in [
        ("@nyx hi", RouteResolution::Mention),
        ("/to nyx hi", RouteResolution::Command),
    ] {
        let activity = message(telegram(), text);
        let outcome = resolve(&def, &input(&def, &activity), true);
        assert_eq!(
            routed_to(outcome),
            Ok(Some((resolution, ids(&[NYX])))),
            "{text}"
        );
        let routed = decision(&def, &input(&def, &activity));
        assert_eq!(&text[usize::from(routed.routing.addressing_bytes)..], "hi");
        println!("{text}: {:?}", routed.trace);
    }
    let def = channel(|value| {
        value["routing"]["rules"] =
            json!([rule("ci", json!({"kinds": ["event"]}), json!(["default"]))]);
    });
    let activity = event("build.done", json!({"cmaegtargets": "reviewer"}));
    let outcome = resolve(&def, &input(&def, &activity), true);
    assert_eq!(
        routed_to(outcome),
        Ok(Some((RouteResolution::EventField, ids(&[REVIEWER]))))
    );
}

#[test]
fn the_first_matching_rule_outranks_the_default() {
    let def = with_message_rule();
    let activity = message(lark(), "hello");
    let routed = decision(&def, &input(&def, &activity));
    println!("rule: {:?}", routed.trace);
    assert_eq!(routed.routing.resolution, RouteResolution::Rule);
    assert_eq!(
        routed.routing.rule_id.as_ref().map(|id| id.as_str()),
        Some("chat")
    );
    assert_eq!(routed.routing.source_id.as_str(), "relay_lark");
    let targets: Vec<String> = routed
        .routing
        .targets
        .iter()
        .map(|t| t.id.to_string())
        .collect();
    assert_eq!(targets, ids(&[REVIEWER]));
}

#[test]
fn the_default_targets_take_what_nothing_else_selects() {
    let def = channel(|value| value["routing"]["default_targets"] = json!([REVIEWER, "default"]));
    let activity = message(telegram(), "hello @nyx");
    let routed = decision(&def, &input(&def, &activity));
    println!("default: {:?}", routed.trace);
    assert_eq!(routed.routing.resolution, RouteResolution::Default);
    assert_eq!(routed.routing.rule_id, None);
    assert_eq!(routed.routing.addressing_bytes, 0);
    assert_eq!(routed.routing.targets[0].id.as_str(), REVIEWER);
    assert_eq!(routed.routing.targets[0].name.as_str(), "reviewer");
    assert_eq!(routed.trace.steps(), [TraceStep::Precedence("default")]);
}

#[test]
fn every_decision_binds_into_a_valid_activity() {
    let def = with_message_rule();
    bound(
        &def,
        &message(telegram(), "@golden_bot /to nyx,main look"),
        None,
    );
    bound(&def, &message(lark(), "@_user_1 @reviewer"), None);
    bound(&def, &message(lark(), "hello"), None);
    bound(
        &def,
        &event("build.done", json!({"cmaegtargets": "main"})),
        None,
    );
    bound(&def, &event("build.done", json!({})), None);
    bound(&def, &click(telegram()), Some(None));
}

#[test]
fn an_inert_rule_is_skipped_and_recorded_as_inactive() {
    let def = channel(|value| {
        value["routing"]["rules"] = json!([rule(
            "slack",
            json!({"sources": ["relay_slack"]}),
            json!([REVIEWER])
        )]);
    });
    let routed = decision(&def, &input(&def, &message(telegram(), "x")));
    assert_eq!(routed.routing.resolution, RouteResolution::Default);
    assert!(matches!(
        routed.trace.steps(),
        [
            TraceStep::RuleTried {
                inactive: true,
                matched: false,
                ..
            },
            TraceStep::Precedence("default")
        ]
    ));
}

#[test]
fn a_click_resolves_only_from_its_card() {
    let def = with_message_rule();
    let activity = click(telegram());
    let refused = resolve(&def, &input(&def, &activity), true);
    assert_eq!(refused, Err(RouteError::InteractionDenied));
    let removed = id("csm_0000000000ff");
    let mut route = input(&def, &activity);
    route.card_consumer = Some(Some(&removed));
    assert_eq!(
        resolve(&def, &route, true),
        Err(RouteError::InteractionDenied)
    );
}

#[test]
fn a_trace_is_bounded_and_carries_no_message_text() {
    let rules: Vec<Value> = (0..32)
        .map(|index| {
            rule(
                &format!("r{index}"),
                json!({"senders": ["someone-else"]}),
                json!([REVIEWER]),
            )
        })
        .collect();
    let def = channel(|value| value["routing"]["rules"] = Value::Array(rules));
    let text = "confidential launch plan";
    let routed = decision(&def, &input(&def, &message(telegram(), text)));
    println!("32 rules: {:?}", routed.trace);
    assert_eq!(routed.routing.resolution, RouteResolution::Default);
    assert_eq!(routed.trace.steps().len(), 16);
    assert!(routed.trace.truncated());
    let shown = format!("{:?}", routed.trace);
    assert!(!shown.contains("confidential") && !shown.contains("someone-else"));
    let addressed = decision(
        &def,
        &input(&def, &message(telegram(), "@reviewer confidential")),
    );
    assert!(!format!("{:?}", addressed.trace).contains("confidential"));
}

#[test]
fn explain_gives_the_trace_of_a_refusal() {
    let def = channel(|value| value["routing"]["consumers"][1]["pending"] = true.into());
    let activity = message(telegram(), "x");
    let mut route = input(&def, &activity);
    let tokens = ids(&["nyx"]);
    route.published = Some((&tokens, causation()));
    assert_eq!(
        resolve(&def, &route, true),
        Err(RouteError::TargetUnknown("nyx".into()))
    );
    assert_eq!(
        explain(&def, &route, true).steps(),
        [TraceStep::Precedence("agent_event")]
    );
}

#[test]
fn unrouted_channels_are_always_legacy_without_a_trace() {
    let def = unrouted();
    let reviewer = id(REVIEWER);
    let tokens = ids(&["reviewer"]);
    let activities = [
        message(telegram(), "@reviewer x"),
        message(lark(), "/to reviewer x"),
        event("build.done", json!({"cmaegtargets": "reviewer"})),
        click(telegram()),
        message(custom(), "x"),
    ];
    for activity in &activities {
        for enabled in [true, false] {
            let mut route = input(&def, activity);
            route.attributes_signed = false;
            assert_eq!(routed_to(resolve(&def, &route, enabled)), Ok(None));
            route.card_consumer = Some(Some(&reviewer));
            route.published = Some((&tokens, causation()));
            assert_eq!(routed_to(resolve(&def, &route, enabled)), Ok(None));
            assert!(explain(&def, &route, enabled).steps().is_empty());
        }
    }
}
