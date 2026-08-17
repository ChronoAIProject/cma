//! Reading this harness's session log as the contract.
//!
//! Every fixture below is a record the pinned harness actually emitted, taken
//! from a real turn driven through the profile this deployment bakes — against
//! a stand-in model endpoint, so nothing was billed and nothing left the
//! machine. Shapes invented here would be a translation tested against itself.

use super::*;

/// One session-log record, as `session.event` delivers it.
fn record(kind: &str, data: serde_json::Value) -> serde_json::Value {
    serde_json::json!({ "seq": 1, "time": "2026-08-17T00:00:00Z", "type": kind, "data": data })
}

/// What a folder makes of a sequence of records.
fn read_all(records: Vec<serde_json::Value>) -> Vec<ConversationUpdate> {
    let mut folder = Folder::default();
    records.iter().flat_map(|r| folder.read(r)).collect()
}

#[test]
fn a_streamed_text_delta_becomes_a_message_chunk() {
    let updates = read_all(vec![record(
        "assistant/chunk",
        serde_json::json!({
            "turn": 1, "step": 1,
            "chunk": { "type": "text-delta", "index": 0, "text": "carried " }
        }),
    )]);
    let [ConversationUpdate::Said(SessionUpdate::AgentMessageChunk(chunk))] = &updates[..] else {
        panic!("expected one message chunk, got {updates:?}");
    };
    assert_eq!(
        serde_json::to_value(&chunk.content).expect("serializes")["text"],
        "carried "
    );
}

#[test]
fn the_chunks_that_are_not_text_say_nothing() {
    // The harness streams block starts and ends, its own usage marker and a
    // finish record down the same channel as text. Only the text is liveness
    // the contract has a word for; the rest arrives again, assembled, in a
    // record that is history.
    let updates = read_all(vec![
        record(
            "assistant/chunk",
            serde_json::json!({ "chunk": { "type": "block-start", "index": 0, "blockType": "text" } }),
        ),
        record(
            "assistant/chunk",
            serde_json::json!({ "chunk": { "type": "block-end", "index": 0 } }),
        ),
        record(
            "assistant/chunk",
            serde_json::json!({ "chunk": { "type": "finish", "reason": { "kind": "stop" } } }),
        ),
    ]);
    assert!(updates.is_empty(), "got {updates:?}");
}

#[test]
fn usage_is_reported_with_the_window_it_was_drawn_from() {
    // The two arrive in separate records — the route's capacity when it is
    // chosen, the step's consumption when the message lands — and the contract
    // wants them together. A usage figure with no window is a number nobody
    // can act on.
    let updates = read_all(vec![
        record(
            "request/context",
            serde_json::json!({ "provider": "chrono", "model": "deepseek-v4-flash", "contextWindow": 262144 }),
        ),
        record(
            "assistant/message",
            serde_json::json!({
                "turn": 1, "step": 1,
                "message": { "role": "assistant", "content": [] },
                "usage": { "inputTokens": 412, "outputTokens": 9 }
            }),
        ),
    ]);
    let [ConversationUpdate::Said(SessionUpdate::UsageUpdate(usage))] = &updates[..] else {
        panic!("expected one usage update, got {updates:?}");
    };
    assert_eq!(usage.used, 421);
    assert_eq!(usage.size, 262_144);
}

#[test]
fn a_message_with_no_accounting_reports_none() {
    // The harness's own contract: usage is absent when the adapter reported
    // none. Inventing a zero would show somebody a session that spent nothing.
    let updates = read_all(vec![record(
        "assistant/message",
        serde_json::json!({ "turn": 1, "step": 1, "message": { "role": "assistant", "content": [] } }),
    )]);
    assert!(updates.is_empty(), "got {updates:?}");
}

#[test]
fn a_tool_call_carries_the_name_the_model_asked_for() {
    let updates = read_all(vec![record(
        "tool/call",
        serde_json::json!({
            "turn": 1, "step": 1, "callId": "call-1", "name": "read",
            "arguments": "{\"path\":\"/home/chrono/notes\"}"
        }),
    )]);
    let [ConversationUpdate::Said(SessionUpdate::ToolCall(call))] = &updates[..] else {
        panic!("expected one tool call, got {updates:?}");
    };
    assert_eq!(call.tool_call_id.0.to_string(), "call-1");
    assert_eq!(call.title, "read");
}

#[test]
fn a_tool_result_closes_the_call_it_belongs_to() {
    let updates = read_all(vec![record(
        "tool/result",
        serde_json::json!({
            "turn": 1, "step": 1,
            "message": { "role": "tool", "content": [{ "id": "call-1", "type": "tool-result" }] }
        }),
    )]);
    let [ConversationUpdate::Said(SessionUpdate::ToolCallUpdate(update))] = &updates[..] else {
        panic!("expected one tool call update, got {updates:?}");
    };
    assert_eq!(update.tool_call_id.0.to_string(), "call-1");
    assert_eq!(update.fields.status, Some(ToolCallStatus::Completed));
}

#[test]
fn a_tool_that_failed_says_so_rather_than_completing() {
    // The contract has a word for it and every interface renders it. Reported
    // as completion, a failed call looks like work that got done.
    let updates = read_all(vec![record(
        "tool/result",
        serde_json::json!({
            "message": { "role": "tool", "content": [{ "id": "call-2" }] },
            "error": { "name": "ENOENT", "code": "NOT_FOUND" }
        }),
    )]);
    let [ConversationUpdate::Said(SessionUpdate::ToolCallUpdate(update))] = &updates[..] else {
        panic!("got {updates:?}");
    };
    assert_eq!(update.fields.status, Some(ToolCallStatus::Failed));
}

#[test]
fn a_completed_turn_ends_the_turn() {
    let updates = read_all(vec![record(
        "turn/end",
        serde_json::json!({ "turn": 1, "reason": { "kind": "completed" } }),
    )]);
    let [ConversationUpdate::TurnEnded(ended)] = &updates[..] else {
        panic!("got {updates:?}");
    };
    assert_eq!(ended.stop_reason, TurnStopReason::EndTurn);
}

#[test]
fn every_way_a_turn_can_fail_is_told_apart_from_finishing() {
    // The harness's reasons are richer than the contract's and several collapse
    // onto one word. Where they do, the safer reading wins: a turn that stopped
    // because something went wrong must not be reported as one that finished.
    for (kind, expected) in [
        ("completed", TurnStopReason::EndTurn),
        ("max-tokens", TurnStopReason::MaxTokens),
        ("aborted", TurnStopReason::Cancelled),
        ("interrupted", TurnStopReason::Cancelled),
        ("blocked", TurnStopReason::Refusal),
        ("error", TurnStopReason::Refusal),
        ("a-reason-a-newer-harness-invented", TurnStopReason::Refusal),
    ] {
        let updates = read_all(vec![record(
            "turn/end",
            serde_json::json!({ "turn": 1, "reason": { "kind": kind } }),
        )]);
        let [ConversationUpdate::TurnEnded(ended)] = &updates[..] else {
            panic!("{kind}: got {updates:?}");
        };
        assert_eq!(ended.stop_reason, expected, "for {kind}");
    }
}

#[test]
fn the_harnesss_own_bookkeeping_crosses_nothing() {
    // Its log is richer than the contract on purpose: request headers, sandbox
    // policy, inbox splices, titles. Approximating any of them would put words
    // in a session's durable log that nobody said.
    let updates = read_all(vec![
        record(
            "permission/preset",
            serde_json::json!({ "preset": "workspace-write" }),
        ),
        record(
            "sandbox/mode",
            serde_json::json!({ "mode": "workspace-write" }),
        ),
        record("approval/policy", serde_json::json!({ "policy": "ask" })),
        record("turn/start", serde_json::json!({ "turn": 1 })),
        record("step/start", serde_json::json!({ "turn": 1, "step": 1 })),
        record("step/end", serde_json::json!({ "turn": 1, "step": 1 })),
        record("session/title", serde_json::json!({ "title": "say hello" })),
        record(
            "request/header",
            serde_json::json!({ "header": {}, "reason": "initial" }),
        ),
        record(
            "agent/inbox/spliced",
            serde_json::json!({ "target": "next-turn" }),
        ),
        record("a-record-a-newer-harness-invented", serde_json::json!({})),
    ]);
    assert!(updates.is_empty(), "got {updates:?}");
}

#[test]
fn a_record_that_is_not_shaped_like_one_is_skipped_rather_than_panicking() {
    // The frames cross a link from a pod that may be running a different
    // version. Nothing here may assume a field is present.
    let updates = read_all(vec![
        serde_json::json!({}),
        serde_json::json!({ "type": "tool/call" }),
        serde_json::json!({ "type": "tool/call", "data": { "callId": "no-name" } }),
        serde_json::json!({ "type": "tool/result", "data": {} }),
        serde_json::json!({ "type": "assistant/chunk", "data": { "chunk": {} } }),
        serde_json::json!({ "type": "turn/end", "data": {} }),
    ]);
    // Only the last yields anything: a turn that ended for no stated reason is
    // still a turn that ended, and the reading that cannot overstate it wins.
    let [ConversationUpdate::TurnEnded(ended)] = &updates[..] else {
        panic!("got {updates:?}");
    };
    assert_eq!(ended.stop_reason, TurnStopReason::Refusal);
}
