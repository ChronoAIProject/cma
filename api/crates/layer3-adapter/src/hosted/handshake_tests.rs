//! Reading a greeting's outcome — the one decision in the handshake that
//! decides whether a live session survives a control-plane restart.
//!
//! Exercised here rather than through a socket. Through a socket the two
//! behaviours are indistinguishable: whichever way silence is read, the link
//! is dead and the next call fails as loss anyway, so the assertion passes for
//! the wrong reason and the guard guards nothing.

use super::greeted;
use crate::AdapterError;

#[test]
fn an_answered_refusal_is_a_live_bridge_and_the_takeover_continues() {
    // #416. `initialize` is once per bridge, and a bot pod outlives the
    // control plane that greeted it — so a refusal here is what taking over a
    // healthy runtime looks like, not what losing one looks like.
    let carried_on = greeted(Err(AdapterError::Rejected(
        "initialize: Already initialized".to_string(),
    )))
    .expect("a refusal proves something was there to refuse");
    assert!(
        carried_on.is_null(),
        "a bridge that refused a second greeting declares nothing, got {carried_on}"
    );
}

#[test]
fn a_refusal_is_read_by_its_kind_and_never_by_its_wording() {
    // A kernel may reword its refusal, change its code, or refuse for a reason
    // nobody here has seen. Matching prose would start destroying sessions the
    // day the wording moved — during a rollout, with nobody watching — and
    // would have covered one kernel and not the other.
    for wording in [
        "initialize: Already initialized",
        "initialize: session already established",
        "initialize: -32603 Internal error",
        "initialize: ",
    ] {
        assert!(
            greeted(Err(AdapterError::Rejected(wording.to_string()))).is_ok(),
            "{wording:?} came back as a frame, so it is a live bridge"
        );
    }
}

#[test]
fn silence_is_not_a_refusal_and_stays_recoverable_as_loss() {
    // The half that keeps genuine loss recoverable. A runtime that has gone
    // answers nothing; reading that as "it refused, carry on" would leave a
    // session pointed at a sandbox that no longer exists and never replace it.
    let failed = greeted(Err(AdapterError::Unavailable(
        "initialize was never answered".to_string(),
    )))
    .expect_err("silence must not be tolerated");
    assert!(
        failed.is_runtime_loss(),
        "silence has to stay classified as loss, got {failed:?}"
    );
}

#[test]
fn a_stated_absence_is_not_a_refusal_either() {
    // `Unsupported` is a property of the image rather than of this instance.
    // It is not evidence of a live bridge, so it must not be swallowed here.
    let failed = greeted(Err(AdapterError::Unsupported("no such method".to_string())))
        .expect_err("an absence is not a greeting");
    assert!(
        matches!(failed, AdapterError::Unsupported(_)),
        "the kind must survive, got {failed:?}"
    );
}

#[test]
fn a_bridge_that_answers_hands_back_what_it_said() {
    // The ordinary path: a fresh bridge declares what it offers, and an
    // own-account session chooses a sign-in from exactly this.
    let hello = greeted(Ok(serde_json::json!({ "authMethods": ["device-code"] })))
        .expect("an answer is an answer");
    assert_eq!(hello["authMethods"][0], "device-code");
}
