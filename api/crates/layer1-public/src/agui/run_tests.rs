//! What a run request means, and how it is refused.

use super::*;

fn parse(body: serde_json::Value) -> RunRequest {
    serde_json::from_value(body).expect("parses")
}

#[test]
fn the_last_user_message_is_the_turn() {
    // A client sends the conversation and the newest thing they said is what
    // this turn is about. Taking the first would replay an old prompt.
    let request = parse(serde_json::json!({
        "threadId": "sesn_1", "runId": "run_1",
        "messages": [
            { "id": "m1", "role": "user", "content": "first" },
            { "id": "m2", "role": "assistant", "content": "answered" },
            { "id": "m3", "role": "user", "content": "second" }
        ]
    }));
    let prompt = request
        .messages
        .iter()
        .rev()
        .find(|m| m.role == "user")
        .and_then(|m| m.content.clone());
    assert_eq!(prompt.as_deref(), Some("second"));
}

#[test]
fn an_answer_that_says_allow_allows() {
    let request = parse(serde_json::json!({
        "threadId": "sesn_1", "runId": "run_2",
        "resume": [{ "interruptId": "apr_7Q2", "status": "resolved",
                     "payload": { "decision": "allow" } }]
    }));
    assert_eq!(decision_of(&request.resume[0]), ApprovalDecision::Allow);
}

#[test]
fn cancelling_is_a_refusal_rather_than_an_absent_answer() {
    // The document says so, and a runtime left with no answer at all waits
    // forever for one.
    let request = parse(serde_json::json!({
        "threadId": "sesn_1", "runId": "run_2",
        "resume": [{ "interruptId": "apr_7Q2", "status": "cancelled" }]
    }));
    assert_eq!(decision_of(&request.resume[0]), ApprovalDecision::Deny);
}

#[test]
fn abandoning_the_turn_is_told_apart_from_refusing_one_action() {
    let request = parse(serde_json::json!({
        "threadId": "sesn_1", "runId": "run_2",
        "resume": [{ "interruptId": "apr_1", "status": "resolved",
                     "payload": { "decision": "abort" } }]
    }));
    assert_eq!(decision_of(&request.resume[0]), ApprovalDecision::Abort);
}

#[test]
fn a_word_this_build_does_not_understand_refuses_rather_than_grants() {
    // The one direction this must fail in. A status or decision from a newer
    // client, read optimistically, would let an agent do something nobody
    // agreed to.
    for body in [
        serde_json::json!({ "interruptId": "a", "status": "resolved",
                            "payload": { "decision": "a-word-from-a-newer-client" } }),
        serde_json::json!({ "interruptId": "a", "status": "a-status-from-a-newer-client" }),
        serde_json::json!({ "interruptId": "a", "status": "resolved" }),
        serde_json::json!({ "interruptId": "a" }),
    ] {
        let item: ResumeItem = serde_json::from_value(body.clone()).expect("parses");
        assert_eq!(decision_of(&item), ApprovalDecision::Deny, "for {body}");
    }
}

#[test]
fn creation_parameters_are_read_only_where_a_thread_could_be_new() {
    // They are frozen at creation, and the handler ignores them on an existing
    // thread. What this checks is that they parse at all — a client that sent
    // them and got a deserialization error would think the request was wrong.
    let request = parse(serde_json::json!({
        "threadId": "sesn_new", "runId": "run_1",
        "messages": [{ "role": "user", "content": "go" }],
        "forwardedProps": { "kernel": "dsh", "environmentProfile": "envp_1",
                            "somethingThisBuildHasNeverHeardOf": true }
    }));
    let props = request.forwarded_props.expect("present");
    assert_eq!(props.kernel.as_deref(), Some("dsh"));
    assert_eq!(props.environment_profile.as_deref(), Some("envp_1"));
}

#[test]
fn a_run_with_neither_a_message_nor_an_answer_has_nothing_to_do() {
    let request = parse(serde_json::json!({ "threadId": "sesn_1", "runId": "run_1" }));
    assert!(request.resume.is_empty());
    assert!(request.messages.is_empty());
}
