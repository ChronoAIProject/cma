//! What this interface says it can do.
//!
//! The document and this handler are two statements of the same contract, and
//! a client reads the second. These check that what it reads is true of the
//! code rather than of the prose.

use super::*;

/// The capability document, parsed.
async fn document() -> serde_json::Value {
    declare().await.0
}

#[tokio::test]
async fn every_event_the_projection_emits_is_declared() {
    // The map is the contract a client branches on. An event emitted but not
    // declared is one a correct client is entitled to have thrown away.
    let doc = document().await;
    let events = doc["events"].as_object().expect("an events map");
    for emitted in [
        "RUN_STARTED",
        "RUN_FINISHED",
        "RUN_ERROR",
        "TEXT_MESSAGE_START",
        "TEXT_MESSAGE_CONTENT",
        "TEXT_MESSAGE_END",
        "TOOL_CALL_START",
        "TOOL_CALL_ARGS",
        "TOOL_CALL_END",
        "TOOL_CALL_RESULT",
        "STATE_DELTA",
    ] {
        assert_eq!(
            events.get(emitted).and_then(serde_json::Value::as_bool),
            Some(true),
            "{emitted} is emitted and must be declared"
        );
    }
}

#[tokio::test]
async fn what_is_not_emitted_is_declared_false_rather_than_omitted() {
    // Omission reads as "this build has not heard of it", which is a different
    // claim from "this build will not send it". A client sizing its handling
    // needs the second.
    let doc = document().await;
    let events = doc["events"].as_object().expect("an events map");
    for absent in ["THINKING_START", "THINKING_END", "STATE_SNAPSHOT"] {
        assert_eq!(
            events.get(absent).and_then(serde_json::Value::as_bool),
            Some(false),
            "{absent} is not emitted and must say so"
        );
    }
}

#[tokio::test]
async fn this_interface_never_claims_to_carry_a_terminal() {
    // There is no honest mapping from an escape sequence to a message event.
    // A client told otherwise would build a terminal on a surface that cannot
    // be one.
    let doc = document().await;
    assert_eq!(doc["features"]["custom"]["chrono:terminal"], false);
    assert_eq!(doc["features"]["transport"]["websocket"], false);
}

#[tokio::test]
async fn live_resource_state_is_declared_before_a_client_receives_it() {
    let doc = document().await;
    assert_eq!(doc["features"]["custom"]["chrono:resources"], true);
}

#[tokio::test]
async fn resumability_is_claimed_because_the_log_is_what_makes_it_true() {
    // Not the stream: a dropped connection is recovered by reading the durable
    // log from the last sequence, which is why this can be claimed at all.
    let doc = document().await;
    assert_eq!(doc["features"]["transport"]["resumable"], true);
    assert_eq!(doc["features"]["state"]["persistentState"], true);
}

#[tokio::test]
async fn the_projection_is_versioned_rather_than_the_protocol() {
    // This protocol is pre-1.0 with no version distinct from its package
    // semver, so there is nothing to claim conformance to. What is versioned
    // is CMA's own projection.
    let doc = document().await;
    let conforms = doc["conformsTo"].as_array().expect("a list");
    assert!(conforms
        .iter()
        .any(|c| c.as_str() == Some("https://spec.chrono-ai.fun/chrono-bot/1.0/conf/agui-core")));
    assert!(
        !conforms.iter().any(|c| c
            .as_str()
            .is_some_and(|s| s.contains("ag-ui.com") || s.contains("agui/v"))),
        "no AG-UI version exists to cite"
    );
}
