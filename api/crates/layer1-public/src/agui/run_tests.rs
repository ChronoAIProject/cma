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

#[test]
fn a_continuation_boundary_is_unnumbered_and_is_not_a_new_turn() {
    let boundary = continuation_started("run_resume", "sesn_1");
    let body = serde_json::to_value(&boundary).expect("the boundary serialises");

    assert_eq!(body["type"], "RUN_STARTED");
    assert_eq!(body["runId"], "run_resume");
    assert_eq!(body["threadId"], "sesn_1");
    assert!(boundary.cursor().is_none());
    assert!(body.get("cma:live").is_none());
    assert!(body.get("timestamp").is_none());
}

#[test]
fn a_durable_batch_keeps_the_complete_first_ending_projection_and_stops() {
    let id = SessionId("sesn_terminal_batch".into());
    let event = |seq, kind| Event {
        session_id: id.clone(),
        seq,
        kind,
        appended_at: "2026-08-26T00:00:00Z".into(),
    };
    let mut pending = std::collections::VecDeque::new();
    let mut seq = 6;

    let ended = queue_durable_batch(
        [
            event(
                7,
                layer2_core::EventKind::ApprovalRequested {
                    request_id: "approval-terminal-batch".into(),
                    action: Some("shell".into()),
                    detail: Some("run a command".into()),
                    locator: None,
                },
            ),
            event(
                8,
                layer2_core::EventKind::StatusChanged {
                    status: layer2_core::SessionStatus::Terminated,
                    stop_reason: Some(layer2_core::StopReason::Error),
                    detail: Some("later failure".into()),
                    capability: None,
                },
            ),
        ],
        &mut pending,
        &mut seq,
        "run_terminal_batch",
        &id.0,
    );

    assert!(ended);
    assert_eq!(seq, 7, "the cursor stops on the event that ended the run");
    assert_eq!(
        pending.len(),
        2,
        "the complete ending projection is kept but the later event is not queued"
    );
    let ending = pending.into_iter().collect::<Vec<_>>();
    assert_eq!(ending[0].kind, "RUN_FINISHED");
    assert_eq!(ending[0].seq, None);
    assert_eq!(ending[1].kind, "STATE_DELTA");
    assert_eq!(ending[1].seq, Some(7));
}

#[test]
fn an_initiating_run_stops_at_rotation_before_same_batch_replacement_events() {
    let id = SessionId("sesn_rotation_batch".into());
    let event = |seq, kind| Event {
        session_id: id.clone(),
        seq,
        kind,
        appended_at: "2026-08-26T00:00:00Z".into(),
    };
    let mut pending = std::collections::VecDeque::new();
    let mut seq = 8;

    let ended = queue_durable_batch(
        [
            event(
                9,
                layer2_core::EventKind::StatusChanged {
                    status: layer2_core::SessionStatus::Rotating,
                    stop_reason: None,
                    detail: Some("moving this session to a replacement runtime".into()),
                    capability: None,
                },
            ),
            event(
                10,
                layer2_core::EventKind::Progress {
                    stage: layer2_core::session::ProvisioningStage::RuntimeRequested,
                },
            ),
            event(
                11,
                layer2_core::EventKind::StatusChanged {
                    status: layer2_core::SessionStatus::Ready,
                    stop_reason: None,
                    detail: None,
                    capability: None,
                },
            ),
        ],
        &mut pending,
        &mut seq,
        "run_rotation_batch",
        &id.0,
    );

    assert!(ended);
    assert_eq!(seq, 9, "the initiating POST stops at the rotation cursor");
    let ending = pending.into_iter().collect::<Vec<_>>();
    assert_eq!(
        ending.iter().map(|frame| frame.kind).collect::<Vec<_>>(),
        ["RUN_ERROR"],
        "replacement progress and ready belong only on the followed session stream"
    );
    assert_eq!(ending[0].seq, Some(9));
    assert_eq!(ending[0].body["cma:returns"], true);
}
