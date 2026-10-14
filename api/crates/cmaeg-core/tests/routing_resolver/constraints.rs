//! The constraints every routed selection meets: pending consumers,
//! `max_targets` and the actor-kind capability.
use cmaeg_core::{
    activity::{Activity, RouteResolution},
    channel::ChannelDefinition,
    ids::ConsumerId,
    routing::{resolve, RouteError, TraceStep},
};
use serde_json::{json, Value};

use super::{
    fixture::{
        causation, channel, click, decision, event, handoff, input, message, routed_to, rule,
        telegram, unchecked,
    },
    policy::{NYX, REVIEWER},
};

type Routed = Result<Option<(RouteResolution, Vec<String>)>, RouteError>;

fn ids(targets: &[&str]) -> Vec<String> {
    targets.iter().map(|id| (*id).to_owned()).collect()
}

fn route(def: &ChannelDefinition, activity: &Activity) -> Routed {
    routed_to(resolve(def, &input(def, activity), true))
}

fn handed_off(def: &ChannelDefinition, tokens: &[&str]) -> Routed {
    let activity = handoff("summarize");
    let tokens = ids(tokens);
    let mut input = input(def, &activity);
    input.published = Some((&tokens, causation()));
    routed_to(resolve(def, &input, true))
}

fn skipped(consumer: &str, reason: &'static str) -> TraceStep {
    TraceStep::Skipped {
        consumer: serde_json::from_value(json!(consumer)).unwrap(),
        reason,
    }
}

/// Rules that send messages, and events, to `targets`.
fn rules_to(targets: Value) -> impl FnOnce(&mut Value) {
    move |value: &mut Value| {
        value["routing"]["rules"] = json!([rule("all", json!({}), targets)]);
    }
}

fn pending_nyx(value: &mut Value) {
    value["routing"]["consumers"][1]["pending"] = true.into();
}

#[test]
fn pending_consumers_are_skipped_in_rules_and_defaults() {
    let def = channel(|value| {
        pending_nyx(value);
        rules_to(json!([NYX, REVIEWER]))(value);
    });
    let routed = decision(&def, &input(&def, &message(telegram(), "x")));
    assert_eq!(routed.routing.targets.len(), 1);
    assert_eq!(routed.routing.targets[0].id.as_str(), REVIEWER);
    assert!(routed.trace.steps().contains(&skipped(NYX, "pending")));
    // Readers refuse a pending default target; a stored one is skipped all the same.
    let def = unchecked(|value| {
        pending_nyx(value);
        value["routing"]["default_targets"] = json!([NYX, "default"]);
    });
    let routed = decision(&def, &input(&def, &message(telegram(), "x")));
    assert_eq!(routed.routing.resolution, RouteResolution::Default);
    assert_eq!(routed.routing.targets[0].id.as_str(), "default");
    assert!(routed.trace.steps().contains(&skipped(NYX, "pending")));
}

#[test]
fn nothing_ready_to_receive_an_event_leaves_it_unavailable() {
    let def = channel(|value| {
        pending_nyx(value);
        rules_to(json!([NYX]))(value);
    });
    assert_eq!(
        route(&def, &message(telegram(), "x")),
        Err(RouteError::ConsumerUnavailable)
    );
}

#[test]
fn a_pending_consumer_named_explicitly_is_unknown() {
    let def = channel(pending_nyx);
    let unknown = Err(RouteError::TargetUnknown("nyx".into()));
    assert_eq!(route(&def, &message(telegram(), "/to nyx x")), unknown);
    let event = event("build.done", json!({"cmaegtargets": "nyx"}));
    assert_eq!(route(&def, &event), unknown);
    assert_eq!(handed_off(&def, &["nyx"]), unknown);
    // A mention of it names no routable consumer, so it stays text.
    let text = route(&def, &message(telegram(), "@nyx x"));
    assert_eq!(
        text,
        Ok(Some((RouteResolution::Default, ids(&["default"]))))
    );
}

#[test]
fn a_click_on_a_pending_consumers_card_is_denied() {
    let def = channel(pending_nyx);
    let nyx: ConsumerId = serde_json::from_value(json!(NYX)).unwrap();
    let activity = click(telegram());
    let mut input = input(&def, &activity);
    input.card_consumer = Some(Some(&nyx));
    assert_eq!(
        resolve(&def, &input, true),
        Err(RouteError::InteractionDenied)
    );
}

#[test]
fn max_targets_is_enforced_for_every_form() {
    let def = channel(|value| value["routing"]["max_targets"] = 2.into());
    let too_many = Err(RouteError::TooManyTargets);
    assert_eq!(
        route(&def, &message(telegram(), "@reviewer @nyx @main x")),
        too_many
    );
    assert_eq!(
        route(&def, &message(telegram(), "/to reviewer,nyx,main x")),
        too_many
    );
    let event = event("build.done", json!({"cmaegtargets": "reviewer,nyx,main"}));
    assert_eq!(route(&def, &event), too_many);
    assert_eq!(handed_off(&def, &["reviewer", "main", "nyx"]), too_many);
    // Readers bound stored lists at write time; a stored list beyond the
    // limit is still refused rather than delivered wider.
    let rule = unchecked(|value| {
        rules_to(json!(["default", REVIEWER]))(value);
        value["routing"]["max_targets"] = 1.into();
    });
    assert_eq!(route(&rule, &message(telegram(), "x")), too_many);
    let default = unchecked(|value| {
        value["routing"]["default_targets"] = json!(["default", REVIEWER]);
        value["routing"]["max_targets"] = 1.into();
    });
    assert_eq!(route(&default, &message(telegram(), "x")), too_many);
}

#[test]
fn duplicates_count_once() {
    let def = channel(|value| value["routing"]["max_targets"] = 1.into());
    let routed = route(&def, &message(telegram(), "/to reviewer,REVIEWER x"));
    assert_eq!(
        routed,
        Ok(Some((RouteResolution::Command, ids(&[REVIEWER]))))
    );
}

#[test]
fn a_custom_event_refuses_a_named_consumer_that_ignores_its_actor() {
    // nyx is a nyxbot consumer and nyxbot is not configured as non-human.
    let def = channel(rules_to(json!(["default", NYX])));
    let named = event("build.done", json!({"cmaegtargets": "nyx"}));
    assert_eq!(
        route(&def, &named),
        Err(RouteError::TargetIgnoresActorKind("nyx".into()))
    );
    let ruled = event("build.done", json!({}));
    let routed = decision(&def, &input(&def, &ruled));
    println!("actor kind: {:?}", routed.trace);
    let targets: Vec<&str> = routed
        .routing
        .targets
        .iter()
        .map(|t| t.id.as_str())
        .collect();
    assert_eq!(targets, ["default"]);
    assert!(routed
        .trace
        .steps()
        .contains(&skipped(NYX, "ignores_actor_kind")));
    // A hand-off's bot actor is held to the same rule.
    assert_eq!(
        handed_off(&def, &["nyx"]),
        Err(RouteError::TargetIgnoresActorKind("nyx".into()))
    );
    // A human may reach any consumer.
    let human = route(&def, &message(telegram(), "@nyx hi"));
    assert_eq!(human, Ok(Some((RouteResolution::Mention, ids(&[NYX])))));
}

#[test]
fn a_consumers_override_beats_its_provider_default() {
    let def = channel(|value| {
        value["routing"]["consumers"][1]["non_human"] = true.into();
        value["routing"]["consumers"][0]["non_human"] = false.into();
        rules_to(json!([REVIEWER, NYX]))(value);
    });
    let named = event("build.done", json!({"cmaegtargets": "nyx"}));
    assert_eq!(
        route(&def, &named),
        Ok(Some((RouteResolution::EventField, ids(&[NYX]))))
    );
    let ruled = decision(&def, &input(&def, &event("build.done", json!({}))));
    let targets: Vec<&str> = ruled
        .routing
        .targets
        .iter()
        .map(|t| t.id.as_str())
        .collect();
    assert_eq!(targets, [NYX]);
    assert!(ruled
        .trace
        .steps()
        .contains(&skipped(REVIEWER, "ignores_actor_kind")));
}

#[test]
fn when_every_target_ignores_the_actor_nothing_accepts_it() {
    let def = channel(rules_to(json!([NYX])));
    let refused = route(&def, &event("build.done", json!({})));
    assert_eq!(refused, Err(RouteError::NoTargetAcceptsActorKind));
    let default = channel(|value| value["routing"]["default_consumer"]["non_human"] = false.into());
    let refused = route(&default, &event("build.done", json!({})));
    assert_eq!(refused, Err(RouteError::NoTargetAcceptsActorKind));
}
