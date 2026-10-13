//! The kill switch fails closed per event (I13): with multi-consumer routing
//! off, a routed channel's event takes the legacy path only when its policy
//! would have given it to `default`, and every resolution kind is covered.
use cmaeg_core::{
    activity::{Activity, RouteResolution},
    channel::{default_receives_all, ChannelDefinition},
    ids::ConsumerId,
    routing::{explain, resolve, Resolution, RouteError, RouteInput, TraceStep},
};
use serde_json::{json, Value};

use super::{
    fixture::{
        causation, channel, click, event, handoff, input, lark, message, non_human_providers, rule,
        telegram,
    },
    policy::{NYX, REVIEWER},
};

const CLOSED: Result<Resolution, RouteError> = Err(RouteError::KillSwitchClosed);

/// Rule `lark-all` sends Lark messages to `{default, reviewer}`; every rule
/// includes `default`, and `reviewer` stays mentionable.
fn rules_include_default() -> ChannelDefinition {
    channel(|value| {
        value["routing"]["rules"] = json!([rule(
            "lark-all",
            json!({"sources": ["relay_lark"]}),
            json!(["default", REVIEWER])
        )]);
    })
}

/// Rule `lark-ops` sends Lark messages to `reviewer` alone.
fn rule_excludes_default() -> ChannelDefinition {
    channel(|value| {
        value["routing"]["rules"] = json!([rule(
            "lark-ops",
            json!({"sources": ["relay_lark"]}),
            json!([REVIEWER])
        )]);
    })
}

/// The outcome with routing off, after printing its trace for review.
fn off(def: &ChannelDefinition, input: &RouteInput<'_>) -> Result<Resolution, RouteError> {
    let trace = explain(def, input, false);
    println!("{:?}", trace.steps());
    assert!(trace.steps().len() <= 16);
    resolve(def, input, false)
}

fn switched_off(def: &ChannelDefinition, activity: &Activity) -> Result<Resolution, RouteError> {
    off(def, &input(def, activity))
}

fn shadow(def: &ChannelDefinition, activity: &Activity) -> Option<TraceStep> {
    explain(def, &input(def, activity), false)
        .steps()
        .iter()
        .find(|step| matches!(step, TraceStep::Shadow { .. }))
        .cloned()
}

fn shadowed(resolution: RouteResolution, outcome: &'static str) -> Option<TraceStep> {
    Some(TraceStep::Shadow {
        resolution,
        outcome,
    })
}

fn targets(value: &str) -> Activity {
    event("build.done", json!({"cmaegtargets": value}))
}

#[test]
fn explicit_addressing_is_legacy_only_when_it_names_exactly_default() {
    let def = rules_include_default();
    // The channel-wide predicate refuses this channel; the switch decides per event.
    assert!(!default_receives_all(&def, non_human_providers()));
    for text in [
        "@reviewer x",
        "/to reviewer x",
        "@main @reviewer x",
        "/to main,reviewer x",
    ] {
        assert_eq!(
            switched_off(&def, &message(telegram(), text)),
            CLOSED,
            "{text}"
        );
    }
    assert_eq!(switched_off(&def, &targets("reviewer")), CLOSED);
    assert_eq!(switched_off(&def, &targets("main,reviewer")), CLOSED);
    for text in [
        "@main x",
        "/to main x",
        "/to default,MAIN x",
        "@golden_bot @main",
    ] {
        assert_eq!(
            switched_off(&def, &message(telegram(), text)),
            Ok(Resolution::Legacy),
            "{text}"
        );
    }
    assert_eq!(switched_off(&def, &targets("main")), Ok(Resolution::Legacy));
    let mention = message(telegram(), "@reviewer x");
    assert_eq!(
        shadow(&def, &mention),
        shadowed(RouteResolution::Mention, "closed")
    );
    let command = message(telegram(), "/to main x");
    assert_eq!(
        shadow(&def, &command),
        shadowed(RouteResolution::Command, "legacy")
    );
    let field = targets("main");
    assert_eq!(
        shadow(&def, &field),
        shadowed(RouteResolution::EventField, "legacy")
    );
}

#[test]
fn rule_and_default_resolutions_are_legacy_when_default_is_among_the_targets() {
    let def = rules_include_default();
    let ruled = message(lark(), "hello");
    assert_eq!(switched_off(&def, &ruled), Ok(Resolution::Legacy));
    assert_eq!(
        shadow(&def, &ruled),
        shadowed(RouteResolution::Rule, "legacy")
    );
    let defaulted = message(telegram(), "hello @reviewer");
    assert_eq!(switched_off(&def, &defaulted), Ok(Resolution::Legacy));
    assert_eq!(
        shadow(&def, &defaulted),
        shadowed(RouteResolution::Default, "legacy")
    );
    assert_eq!(
        switched_off(&def, &event("build.done", json!({}))),
        Ok(Resolution::Legacy)
    );
    let wide = channel(|value| value["routing"]["default_targets"] = json!([REVIEWER, "default"]));
    assert_eq!(
        switched_off(&wide, &message(telegram(), "x")),
        Ok(Resolution::Legacy)
    );
}

#[test]
fn a_rule_that_excludes_default_closes_only_the_events_it_matches() {
    let def = rule_excludes_default();
    let matched = message(lark(), "deploy now");
    assert_eq!(switched_off(&def, &matched), CLOSED);
    assert_eq!(
        shadow(&def, &matched),
        shadowed(RouteResolution::Rule, "closed")
    );
    assert_eq!(
        switched_off(&def, &message(telegram(), "deploy now")),
        Ok(Resolution::Legacy)
    );
    assert_eq!(
        switched_off(&def, &event("build.done", json!({}))),
        Ok(Resolution::Legacy)
    );
    let elsewhere = channel(|value| value["routing"]["default_targets"] = json!([REVIEWER]));
    assert_eq!(switched_off(&elsewhere, &message(telegram(), "x")), CLOSED);
}

#[test]
fn any_routing_error_closes() {
    let def = rules_include_default();
    for text in ["/to ghost x", "/to main,ghost x"] {
        assert_eq!(
            switched_off(&def, &message(telegram(), text)),
            CLOSED,
            "{text}"
        );
    }
    for value in [json!(7), json!("main,,main"), json!("ghost")] {
        let activity = event("build.done", json!({"cmaegtargets": value}));
        assert_eq!(switched_off(&def, &activity), CLOSED, "{value}");
    }
    let field = targets("main");
    let mut unsigned = input(&def, &field);
    unsigned.attributes_signed = false;
    assert_eq!(off(&def, &unsigned), CLOSED);
    let refused = channel(|value| value["routing"]["addressing"] = json!({"event_field": false}));
    assert_eq!(switched_off(&refused, &field), CLOSED);
    assert_eq!(
        shadow(&refused, &field),
        shadowed(RouteResolution::EventField, "closed")
    );
    let quiet = channel(|value| value["routing"]["default_consumer"]["mentionable"] = false.into());
    assert_eq!(
        switched_off(&quiet, &message(telegram(), "@main x")),
        CLOSED
    );
    let tight = channel(|value| value["routing"]["max_targets"] = 1.into());
    assert_eq!(
        switched_off(&tight, &message(telegram(), "@main @reviewer")),
        CLOSED
    );
}

#[test]
fn unavailable_or_incapable_defaults_close() {
    let pending = channel(|value| {
        value["routing"]["consumers"][1]["pending"] = true.into();
        value["routing"]["rules"] = json!([rule("all", json!({}), json!([NYX]))]);
    });
    let message = message(telegram(), "x");
    assert_eq!(switched_off(&pending, &message), CLOSED);
    assert_eq!(
        shadow(&pending, &message),
        shadowed(RouteResolution::Rule, "closed")
    );
    // default would skip a system actor it does not answer, so the event
    // would reach only reviewer.
    let skipped = channel(|value| {
        value["routing"]["default_consumer"]["non_human"] = false.into();
        value["routing"]["default_targets"] = json!(["default", REVIEWER]);
    });
    assert_eq!(
        switched_off(&skipped, &event("build.done", json!({}))),
        CLOSED
    );
    assert_eq!(switched_off(&skipped, &targets("main")), CLOSED);
}

#[test]
fn only_defaults_cards_take_the_legacy_path() {
    let def = rules_include_default();
    let activity = click(telegram());
    let mut route = input(&def, &activity);
    route.card_consumer = Some(None);
    assert_eq!(off(&def, &route), Ok(Resolution::Legacy));
    let default: ConsumerId = serde_json::from_value(json!("default")).unwrap();
    route.card_consumer = Some(Some(&default));
    assert_eq!(off(&def, &route), Ok(Resolution::Legacy));
    let reviewer: ConsumerId = serde_json::from_value(json!(REVIEWER)).unwrap();
    route.card_consumer = Some(Some(&reviewer));
    assert_eq!(off(&def, &route), Err(RouteError::InteractionDenied));
    let steps = explain(&def, &route, false);
    assert_eq!(
        steps.steps(),
        [
            TraceStep::Precedence("kill_switch"),
            shadowed(RouteResolution::Interaction, "closed").unwrap()
        ]
    );
    // A click the caller did not resolve to a card never falls through.
    assert_eq!(switched_off(&def, &activity), CLOSED);
}

#[test]
fn hand_offs_are_refused() {
    let def = rules_include_default();
    let activity = handoff("summarize");
    let tokens = vec!["main".to_owned()];
    let mut route = input(&def, &activity);
    route.published = Some((&tokens, causation()));
    assert_eq!(off(&def, &route), CLOSED);
    let steps = explain(&def, &route, false);
    assert_eq!(
        steps.steps()[1],
        shadowed(RouteResolution::AgentEvent, "closed").unwrap()
    );
}

#[test]
fn turning_a_form_off_makes_its_text_ordinary() {
    let no_mentions = channel(|value| value["routing"]["addressing"] = json!({"mentions": false}));
    let mention = message(telegram(), "@reviewer x");
    assert_eq!(switched_off(&no_mentions, &mention), Ok(Resolution::Legacy));
    assert_eq!(
        shadow(&no_mentions, &mention),
        shadowed(RouteResolution::Default, "legacy")
    );
}

#[test]
fn shadow_traces_are_bounded_and_carry_no_message_text() {
    let rules: Vec<Value> = (0..32)
        .map(|index| {
            rule(
                &format!("r{index}"),
                json!({"chats": ["elsewhere"]}),
                json!([REVIEWER]),
            )
        })
        .collect();
    let def = channel(|value| value["routing"]["rules"] = Value::Array(rules));
    let activity = message(telegram(), "quarterly numbers");
    assert_eq!(switched_off(&def, &activity), Ok(Resolution::Legacy));
    let steps = explain(&def, &input(&def, &activity), false);
    assert_eq!(steps.steps().len(), 16);
    assert!(steps.truncated());
    assert!(!format!("{steps:?}").contains("quarterly"));
}
