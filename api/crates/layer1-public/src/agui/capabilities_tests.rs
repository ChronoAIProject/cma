//! What this interface says it can do.
//!
//! The document and this handler are two statements of the same contract, and
//! a client reads the second. These check that what it reads is true of the
//! code rather than of the prose.

use super::*;

/// The capability document, parsed.
async fn served_document() -> serde_json::Value {
    declare().await.0
}

fn documented_capability(source: &str) -> Result<serde_json::Value, String> {
    const SECTION: &str = "## 6. Capability document";
    const JSON_FENCE: &str = "```json\n";

    let remainder = source
        .split_once(SECTION)
        .map(|(_, section)| section)
        .ok_or_else(|| format!("docs/API_AGUI.md has no {SECTION:?} section"))?;
    let section_end = ["\n# ", "\n## "]
        .into_iter()
        .filter_map(|heading| remainder.find(heading))
        .min()
        .unwrap_or(remainder.len());
    let section = &remainder[..section_end];
    let block = section
        .split_once(JSON_FENCE)
        .map(|(_, block)| block)
        .and_then(|block| block.split_once("\n```").map(|(json, _)| json))
        .ok_or_else(|| "docs/API_AGUI.md section 6 has no fenced JSON document".to_string())?;

    serde_json::from_str(block)
        .map_err(|error| format!("docs/API_AGUI.md section 6 is not JSON: {error}"))
}

#[test]
fn deleting_the_documented_capability_block_fails_loudly() {
    for next_heading in ["## 7. Notes", "# Replacement document"] {
        let source = format!(
            "## 6. Capability document\n\nNo example.\n\n{next_heading}\n\n```json\n{{}}\n```"
        );
        let error = documented_capability(&source)
            .expect_err("a later section's JSON must not look like the section 6 contract");
        assert!(error.contains("no fenced JSON document"), "{error}");
    }
}

#[tokio::test]
async fn the_documented_capability_is_the_served_document() {
    let documented = documented_capability(include_str!("../../../../../docs/API_AGUI.md"))
        .expect("section 6 contains the capability JSON contract");
    assert_eq!(documented, served_document().await);
}

#[tokio::test]
async fn trigger_companions_are_declared_read_only() {
    let document = served_document().await;
    let trigger = &document["features"]["custom"]["cma:trigger"];
    assert_eq!(trigger["preview"], true);
    assert_eq!(trigger["launchSnapshot"], true);
    assert_eq!(trigger["progressReplay"], true);
    assert_eq!(trigger["diffReview"], true);
    assert_eq!(trigger["mutations"], false);
}

#[tokio::test]
async fn every_event_the_projection_emits_is_declared() {
    // The map is the contract a client branches on. An event emitted but not
    // declared is one a correct client is entitled to have thrown away.
    let doc = served_document().await;
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
    let doc = served_document().await;
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
    let doc = served_document().await;
    assert_eq!(doc["features"]["custom"]["cma:terminal"], false);
    assert_eq!(doc["features"]["transport"]["websocket"], false);
}

#[tokio::test]
async fn live_resource_state_is_declared_before_a_client_receives_it() {
    let doc = served_document().await;
    assert_eq!(doc["features"]["custom"]["cma:resources"], true);
}

#[tokio::test]
async fn resumability_is_claimed_because_the_log_is_what_makes_it_true() {
    // Not the stream: a dropped connection is recovered by reading the durable
    // log from the last sequence, which is why this can be claimed at all.
    let doc = served_document().await;
    assert_eq!(doc["features"]["transport"]["resumable"], true);
    assert_eq!(doc["features"]["state"]["persistentState"], true);
}

#[tokio::test]
async fn the_projection_is_versioned_rather_than_the_protocol() {
    // This protocol is pre-1.0 with no version distinct from its package
    // semver, so there is nothing to claim conformance to. What is versioned
    // is CMA's own projection.
    let doc = served_document().await;
    let conforms = doc["conformsTo"].as_array().expect("a list");
    assert!(conforms
        .iter()
        .any(|c| c.as_str() == Some("https://spec.chrono-ai.fun/cma/1.0/conf/agui-core")));
    assert!(
        !conforms.iter().any(|c| c
            .as_str()
            .is_some_and(|s| s.contains("ag-ui.com") || s.contains("agui/v"))),
        "no AG-UI version exists to cite"
    );
}
