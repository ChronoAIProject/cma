//! The deciding stage survives both parser errors and bounded trace truncation.
use super::{fixture::*, policy::REVIEWER};
use cmaeg_core::{
    activity::RouteResolution,
    routing::{explain, resolve, MAX_TRACE_STEPS},
};
use serde_json::json;

#[test]
fn early_refusals_keep_the_attempted_stage_without_tokens() {
    let def = channel(|_| {});
    for (activity, stage) in [
        (
            message(telegram(), "/to unknown-private-token x"),
            RouteResolution::Command,
        ),
        (
            event("build.done", json!({"cmaegtargets": 1})),
            RouteResolution::EventField,
        ),
        (click(telegram()), RouteResolution::Interaction),
    ] {
        let input = input(&def, &activity);
        for enabled in [false, true] {
            assert!(resolve(&def, &input, enabled).is_err());
            let trace = explain(&def, &input, enabled);
            assert_eq!(trace.selected_resolution(), Some(stage));
            assert!(!format!("{trace:?}").contains("unknown-private-token"));
        }
    }
    let hidden = channel(|raw| raw["routing"]["consumers"][0]["mentionable"] = false.into());
    let activity = message(telegram(), "@reviewer x");
    let input = input(&hidden, &activity);
    assert!(resolve(&hidden, &input, true).is_err());
    assert_eq!(
        explain(&hidden, &input, true).selected_resolution(),
        Some(RouteResolution::Mention)
    );
}

#[test]
fn fallthrough_and_truncation_keep_the_actual_rule_or_default_stage() {
    for matches in [false, true] {
        let def = channel(|raw| {
            let mut rules: Vec<_> = (0..31)
                .map(|i| {
                    rule(
                        &format!("r{i}"),
                        json!({"chats":["elsewhere"]}),
                        json!(["default"]),
                    )
                })
                .collect();
            if matches {
                rules.push(rule("last", json!({}), json!([REVIEWER])));
            }
            raw["routing"]["rules"] = rules.into();
        });
        let activity = message(telegram(), "@unknown-falls-through x");
        let input = input(&def, &activity);
        for enabled in [false, true] {
            let trace = explain(&def, &input, enabled);
            assert!(trace.truncated());
            assert_eq!(trace.steps().len(), MAX_TRACE_STEPS);
            assert_eq!(
                trace.selected_resolution(),
                Some(if matches {
                    RouteResolution::Rule
                } else {
                    RouteResolution::Default
                })
            );
            assert_eq!(resolve(&def, &input, enabled).is_ok(), enabled || !matches);
        }
    }
}

#[test]
fn card_and_publish_refusals_keep_their_precedence() {
    let def = channel(|_| {});
    let activity = click(telegram());
    let missing = cmaeg_core::ids::ConsumerId::parse("csm_00000000000f").unwrap();
    let mut click_input = input(&def, &activity);
    click_input.card_consumer = Some(Some(&missing));
    let tokens = vec!["unknown-private-token".into()];
    let handoff = handoff("private payload");
    let mut published = input(&def, &handoff);
    published.published = Some((&tokens, causation()));
    for (input, stage) in [
        (click_input, RouteResolution::Interaction),
        (published, RouteResolution::AgentEvent),
    ] {
        for enabled in [false, true] {
            assert!(resolve(&def, &input, enabled).is_err());
            assert_eq!(
                explain(&def, &input, enabled).selected_resolution(),
                Some(stage)
            );
        }
    }
    let legacy = unrouted();
    assert_eq!(
        explain(&legacy, &input(&legacy, &activity), true).selected_resolution(),
        None
    );
}
