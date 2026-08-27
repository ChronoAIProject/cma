//! Evidence classification shared by every hosted ACP kernel.
//!
//! The JSON values are ACP v1 update fixtures, not kernel-specific events.
//! Codex and Claude Code both enter through `open_acp_conversation`, so this is
//! the semantic boundary their adapter-local binding tests select.

use super::acp::Outcome;
use crate::{
    ConversationId, ConversationUpdate, PermissionRequest, RequestPermissionRequest, SessionUpdate,
    ToolCallId, ToolCallUpdate, ToolCallUpdateFields, TurnEvidence, TurnScopedEvidence,
};

fn said_fixture(value: serde_json::Value) -> ConversationUpdate {
    ConversationUpdate::Said(
        serde_json::from_value::<SessionUpdate>(value).expect("the ACP fixture is readable"),
    )
}

fn permission_fixture() -> ConversationUpdate {
    ConversationUpdate::Asked(PermissionRequest {
        id: "permission-1".into(),
        locator: None,
        request: RequestPermissionRequest::new(
            ConversationId::new("conversation-1"),
            ToolCallUpdate::new(ToolCallId::new("call-1"), ToolCallUpdateFields::new()),
            Vec::new(),
        ),
    })
}

fn outcome_update(frame: serde_json::Value) -> ConversationUpdate {
    match super::turn::turn_outcome(frame) {
        Outcome::Ended(response) => ConversationUpdate::TurnEnded(response),
        Outcome::Failed {
            message,
            capability,
        } => ConversationUpdate::Failed {
            message,
            capability,
        },
    }
}

#[test]
fn acp_turn_activity_has_one_closed_positive_allowlist() {
    let fixtures = vec![
        (
            said_fixture(serde_json::json!({
                "sessionUpdate": "agent_message_chunk",
                "content": { "type": "text", "text": "working" }
            })),
            TurnScopedEvidence::AgentMessage,
        ),
        (
            said_fixture(serde_json::json!({
                "sessionUpdate": "tool_call",
                "toolCallId": "call-1",
                "title": "read"
            })),
            TurnScopedEvidence::ToolCallStarted,
        ),
        (
            said_fixture(serde_json::json!({
                "sessionUpdate": "tool_call_update",
                "toolCallId": "call-1",
                "status": "completed"
            })),
            TurnScopedEvidence::ToolCallUpdated,
        ),
        (permission_fixture(), TurnScopedEvidence::ApprovalRequested),
    ];

    let classified: Vec<_> = fixtures
        .into_iter()
        .map(|(update, expected)| {
            assert_eq!(
                update.turn_evidence(),
                Some(TurnEvidence::Activity(expected))
            );
            expected
        })
        .collect();
    assert_eq!(classified, TurnScopedEvidence::ALL);
}

#[test]
fn acp_initialization_and_reconnect_traffic_never_acknowledges_a_prompt() {
    let initialization = [
        serde_json::json!({
            "sessionUpdate": "available_commands_update",
            "availableCommands": []
        }),
        serde_json::json!({
            "sessionUpdate": "current_mode_update",
            "currentModeId": "agent"
        }),
        serde_json::json!({
            "sessionUpdate": "config_option_update",
            "configOptions": []
        }),
        serde_json::json!({
            "sessionUpdate": "usage_update",
            "used": 0,
            "size": 200000
        }),
    ];

    // The second pass represents the same declarations replayed while an ACP
    // bridge reconnects. Ordering after a dispatch fence does not change what
    // these protocol events prove.
    for delivery in 0..2 {
        for fixture in initialization.iter().cloned() {
            assert_eq!(
                said_fixture(fixture).turn_evidence(),
                None,
                "initialization delivery {delivery} became execution evidence"
            );
        }
    }

    assert_eq!(
        ConversationUpdate::SignInRequired {
            url: "https://example.invalid/device".into(),
            code: Some("ABCD".into()),
            message: None,
        }
        .turn_evidence(),
        None
    );
    assert_eq!(ConversationUpdate::Lost { frames: 3 }.turn_evidence(), None);
}

#[test]
fn acp_terminal_success_and_failure_are_typed_separately() {
    let completed = outcome_update(serde_json::json!({
        "jsonrpc": "2.0",
        "id": 7,
        "result": { "stopReason": "end_turn" }
    }));
    assert_eq!(
        completed.turn_evidence(),
        Some(TurnEvidence::TerminalResult)
    );

    let failed = outcome_update(serde_json::json!({
        "jsonrpc": "2.0",
        "id": 8,
        "error": { "code": -32000, "message": "model unavailable" }
    }));
    assert_eq!(failed.turn_evidence(), Some(TurnEvidence::TerminalFailure));
}

#[test]
fn hosted_activity_is_evidence_only_while_this_bridge_owns_a_prompt_waiter() {
    let activity = vec![
        said_fixture(serde_json::json!({
            "sessionUpdate": "agent_message_chunk",
            "content": { "type": "text", "text": "working" }
        })),
        said_fixture(serde_json::json!({
            "sessionUpdate": "tool_call",
            "toolCallId": "call-1",
            "title": "read"
        })),
        said_fixture(serde_json::json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": "call-1",
            "status": "completed"
        })),
        permission_fixture(),
    ];

    for update in activity {
        let expected = update
            .turn_evidence()
            .expect("the fixture is on the closed activity allowlist");
        let startup_or_replay = super::inbound::scope_activity(update.clone(), false);
        assert!(matches!(
            startup_or_replay,
            ConversationUpdate::WithoutPromptEvidence(_)
        ));
        assert_eq!(startup_or_replay.turn_evidence(), None);

        let live_prompt = super::inbound::scope_activity(update, true);
        assert_eq!(live_prompt.turn_evidence(), Some(expected));
    }
}

#[test]
fn local_turn_failures_are_not_terminal_prompt_evidence() {
    for message in [
        "the agent did not answer this turn",
        "the agent stopped answering before the turn finished",
        "the prompt could not be sent to the agent",
    ] {
        let update = super::turn::scoped_turn_outcome(
            Outcome::Failed {
                message: message.to_string(),
                capability: None,
            },
            false,
        );
        assert!(matches!(
            update,
            ConversationUpdate::WithoutPromptEvidence(_)
        ));
        assert_eq!(update.turn_evidence(), None);
    }
}

#[test]
fn matched_prompt_response_remains_terminal_prompt_evidence() {
    let outcome = super::turn::turn_outcome(serde_json::json!({
        "jsonrpc": "2.0",
        "id": 9,
        "result": { "stopReason": "end_turn" }
    }));
    let update = super::turn::scoped_turn_outcome(outcome, true);
    assert_eq!(update.turn_evidence(), Some(TurnEvidence::TerminalResult));
}
