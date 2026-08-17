//! Turning an owner's decision into an answer a bridge understands.
//!
//! This is where a wrong answer grants more than somebody asked for, or sends
//! a decision to the wrong place. The options a bridge offers are its own and
//! differ per request, so every case here is about matching an intent against
//! what was actually on offer rather than against a fixed set.

use super::outcome_for;
use crate::{PermissionDecision, PermissionOption, PermissionOptionId, PermissionOptionKind};

fn offering(kinds: &[PermissionOptionKind]) -> Vec<PermissionOption> {
    kinds
        .iter()
        .enumerate()
        .map(|(n, kind)| {
            PermissionOption::new(
                PermissionOptionId::new(format!("option-{n}")),
                format!("{kind:?}"),
                *kind,
            )
        })
        .collect()
}

#[test]
fn allowing_selects_an_option_the_bridge_actually_offered() {
    // The options are the bridge's own and differ per request, so an intent is
    // matched against what was on offer rather than against a fixed set.
    let outcome = outcome_for(
        PermissionDecision::Allow,
        &offering(&[
            PermissionOptionKind::RejectOnce,
            PermissionOptionKind::AllowOnce,
        ]),
    );
    assert_eq!(outcome["outcome"], "selected");
    assert_eq!(outcome["optionId"], "option-1");
}

#[test]
fn allowing_falls_back_to_a_standing_permission_when_that_is_all_there_is() {
    let outcome = outcome_for(
        PermissionDecision::Allow,
        &offering(&[PermissionOptionKind::AllowAlways]),
    );
    assert_eq!(outcome["outcome"], "selected");
    assert_eq!(outcome["optionId"], "option-0");
}

#[test]
fn refusing_selects_a_refusal() {
    let outcome = outcome_for(
        PermissionDecision::Deny,
        &offering(&[
            PermissionOptionKind::AllowOnce,
            PermissionOptionKind::RejectOnce,
        ]),
    );
    assert_eq!(outcome["outcome"], "selected");
    assert_eq!(outcome["optionId"], "option-1");
}

#[test]
fn aborting_cancels_rather_than_choosing_a_refusal() {
    // Refusing one action and abandoning the turn are different intentions,
    // and only cancellation stops the agent trying something else.
    let outcome = outcome_for(
        PermissionDecision::Abort,
        &offering(&[
            PermissionOptionKind::AllowOnce,
            PermissionOptionKind::RejectOnce,
        ]),
    );
    assert_eq!(outcome["outcome"], "cancelled");
}

#[test]
fn an_intent_with_nothing_to_match_stops_the_turn() {
    // The safe direction when the answer was "no" and the bridge offered no
    // way to say it: cancelling stops the action, where selecting nothing
    // would leave the runtime blocked forever.
    let outcome = outcome_for(
        PermissionDecision::Deny,
        &offering(&[PermissionOptionKind::AllowOnce]),
    );
    assert_eq!(outcome["outcome"], "cancelled");
}

#[test]
fn allowing_with_no_option_to_allow_with_also_stops_the_turn() {
    // Rather than picking a refusal and reporting it as consent.
    let outcome = outcome_for(
        PermissionDecision::Allow,
        &offering(&[PermissionOptionKind::RejectOnce]),
    );
    assert_eq!(outcome["outcome"], "cancelled");
}
