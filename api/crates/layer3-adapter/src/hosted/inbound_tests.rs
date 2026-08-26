//! Turning an owner's decision into an answer a bridge understands.
//!
//! This is where a wrong answer grants more than somebody asked for, or sends
//! a decision to the wrong place. The options a bridge offers are its own and
//! differ per request, so every case here is about matching an intent against
//! what was actually on offer rather than against a fixed set.

use super::{blocked_from_locator, outcome_for, permission_id, permission_locator, sign_in_from};
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

#[test]
fn duplicate_delivery_has_one_stable_public_id() {
    let call = serde_json::json!(73);
    let first = vec![PermissionOption::new(
        PermissionOptionId::new("allow-once"),
        "Allow this command".to_string(),
        PermissionOptionKind::AllowOnce,
    )];
    let replayed = vec![PermissionOption::new(
        PermissionOptionId::new("allow-once"),
        "A reworded label".to_string(),
        PermissionOptionKind::AllowOnce,
    )];

    let first_locator = permission_locator("conversation-1", &call, &first);
    let replayed_locator = permission_locator("conversation-1", &call, &replayed);
    assert_eq!(
        permission_id(&first_locator),
        permission_id(&replayed_locator)
    );
    assert_eq!(first_locator, replayed_locator);
    assert!(
        !first_locator.as_str().contains("command") && !first_locator.as_str().contains("reworded"),
        "option labels are not delivery metadata"
    );
}

#[test]
fn a_locator_cannot_be_reused_under_another_public_id() {
    let locator = permission_locator(
        "conversation-1",
        &serde_json::json!(91),
        &offering(&[PermissionOptionKind::AllowOnce]),
    );
    let Err(error) = blocked_from_locator("a-different-request", &locator, "conversation-1") else {
        panic!("the fingerprint must bind the locator to its public id");
    };
    assert!(matches!(error, crate::AdapterError::Rejected(_)));
}

#[test]
fn a_locator_cannot_be_routed_on_another_conversation() {
    let locator = permission_locator(
        "conversation-1",
        &serde_json::json!(92),
        &offering(&[PermissionOptionKind::AllowOnce]),
    );
    let id = permission_id(&locator);
    let Err(error) = blocked_from_locator(&id, &locator, "conversation-2") else {
        panic!("the locator's conversation scope must be enforced");
    };
    assert!(matches!(error, crate::AdapterError::Rejected(_)));
}

/// Reading a sign-in the agent is waiting on.
///
/// What a client shows somebody is decided here, and getting it wrong shows
/// them a card they cannot act on.
#[test]
fn a_sign_in_carries_where_to_go_and_what_to_type() {
    let params = serde_json::json!({
        "mode": "url",
        "url": "https://example.invalid/activate",
        "userCode": "ABCD-EFGH",
        "message": "Sign in and enter this code: ABCD-EFGH",
    });
    let (url, code, message) = sign_in_from(Some(&params)).expect("a complete request is read");
    assert_eq!(url, "https://example.invalid/activate");
    assert_eq!(code.as_deref(), Some("ABCD-EFGH"));
    assert!(message.expect("the sentence travels").contains("code"));
}

#[test]
fn a_request_with_no_address_is_read_as_nothing() {
    // A sign-in card with nowhere to go is worse than none: it tells somebody
    // to act and gives them no way to. Refused here rather than drawn empty.
    assert!(sign_in_from(Some(&serde_json::json!({ "message": "sign in" }))).is_none());
    assert!(sign_in_from(None).is_none());
}

#[test]
fn a_code_is_taken_only_from_where_it_is_declared() {
    // Some bridges name it and some only put it in the sentence. Parsing it
    // out of prose would break the first time the wording changed, and the
    // sentence is shown whole either way.
    let only_prose = serde_json::json!({
        "url": "https://example.invalid/activate",
        "message": "Enter WXYZ-1234 to continue",
    });
    let (_, code, message) = sign_in_from(Some(&only_prose)).expect("still readable");
    assert!(code.is_none(), "no code was declared, so none is claimed");
    assert!(message
        .expect("the sentence carries it")
        .contains("WXYZ-1234"));
}

#[test]
fn the_alternate_spelling_of_the_code_field_is_accepted() {
    let params = serde_json::json!({ "url": "https://example.invalid/a", "code": "ZZZZ" });
    let (_, code, _) = sign_in_from(Some(&params)).expect("readable");
    assert_eq!(code.as_deref(), Some("ZZZZ"));
}
