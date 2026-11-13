use super::*;
use crate::sandbox::RecordedToolOutput;

fn row(seq: u64, kind: AgentLogKind) -> AgentLogEntry {
    AgentLogEntry {
        agent_id: AgentId("agt_index".into()),
        seq,
        appended_at: "2026-11-11T00:00:00Z".into(),
        kind,
    }
}
fn index() -> AgentTurnOutputIndex {
    AgentTurnOutputIndex::start(&row(
        1,
        AgentLogKind::TurnStarted {
            turn_sequence: 1,
            origin_key: None,
            model: Some("retained".into()),
        },
    ))
    .unwrap()
}
fn start(call_id: &str, status: &str) -> AgentLogKind {
    AgentLogKind::ToolCallStarted {
        call_id: call_id.into(),
        title: "execute".into(),
        kind: "execute".into(),
        status: status.into(),
    }
}
fn update(call_id: &str, status: &str, output: Option<&str>) -> AgentLogKind {
    AgentLogKind::ToolCallUpdated {
        call_id: call_id.into(),
        title: None,
        status: status.into(),
        output: output.map(|text| RecordedToolOutput {
            text: text.into(),
            truncated: false,
        }),
    }
}

#[test]
fn long_turn_counts_empty_and_nonempty_messages_without_retaining_payloads() {
    let mut state = index();
    for i in 0..100_000 {
        let change = state
            .observe(
                &row(
                    i + 2,
                    AgentLogKind::AgentMessage {
                        content: if i % 2 == 0 {
                            String::new()
                        } else {
                            " ".repeat(10)
                        },
                    },
                ),
                AgentOutputTool::None,
            )
            .unwrap()
            .change
            .unwrap();
        assert_eq!(change.ordinal, i);
        assert_eq!(change.message_ordinal, Some(i));
        assert_eq!(change.item_ordinal, i);
    }
    assert_eq!(state.changes, 100_000);
    assert_eq!(state.totals.nonempty_messages, 50_000);
    assert!(serde_json::to_vec(&state).unwrap().len() < 400);
}

#[test]
fn duplicate_and_late_tool_updates_preserve_first_terminal_evidence() {
    let mut state = index();
    let first = state
        .observe(
            &row(2, start("tool", "in_progress")),
            AgentOutputTool::Absent,
        )
        .unwrap();
    let mut tool = first.tool.unwrap();
    assert_eq!(first.change.unwrap().after.tool_starts, 1);
    assert!(state
        .observe(
            &row(3, start("tool", "completed")),
            AgentOutputTool::Stored(&tool)
        )
        .unwrap()
        .change
        .is_none());
    let before_done = state
        .observe(
            &row(4, update("tool", "in_progress", Some("first"))),
            AgentOutputTool::Stored(&tool),
        )
        .unwrap();
    assert!(before_done.change.is_none());
    tool = before_done.tool.unwrap();
    let done = state
        .observe(
            &row(5, update("tool", "completed", None)),
            AgentOutputTool::Stored(&tool),
        )
        .unwrap();
    let immutable = done.change.unwrap();
    tool = done.tool.unwrap();
    assert_eq!(immutable.tool.as_ref().unwrap().output_sequence, Some(4));
    assert_eq!(immutable.after.tool_finishes, 1);
    let later = state
        .observe(
            &row(6, update("tool", "failed", Some("later"))),
            AgentOutputTool::Stored(&tool),
        )
        .unwrap();
    assert!(later.change.is_none());
    assert_eq!(later.tool.unwrap().output_sequence, Some(6));
    assert_eq!(immutable.tool.unwrap().output_sequence, Some(4));
    assert_eq!(state.changes, 2);
}

#[test]
fn terminal_start_records_both_transitions_and_missing_lookup_refuses_unchanged() {
    let mut state = index();
    let before = state.clone();
    assert!(state
        .observe(&row(2, start("tool", "completed")), AgentOutputTool::None)
        .is_err());
    assert_eq!(state, before);
    let delta = state
        .observe(&row(2, start("tool", "completed")), AgentOutputTool::Absent)
        .unwrap();
    let change = delta.change.unwrap();
    assert_eq!(
        (change.after.tool_starts, change.after.tool_finishes),
        (1, 1)
    );
    assert_eq!(change.tool.unwrap().terminal_sequence, Some(2));
}

#[test]
fn wrong_source_tool_and_nonconsecutive_rows_refuse_without_mutating_counters() {
    let mut state = index();
    let mut tool = state
        .observe(
            &row(2, start("tool", "in_progress")),
            AgentOutputTool::Absent,
        )
        .unwrap()
        .tool
        .unwrap();
    let before = state.clone();
    tool.turn_sequence = 99;
    assert!(state
        .observe(
            &row(3, update("tool", "completed", None)),
            AgentOutputTool::Stored(&tool)
        )
        .is_err());
    assert_eq!(state, before);
    let mut wrong = row(3, AgentLogKind::AgentMessage { content: "".into() });
    wrong.agent_id = AgentId("wrong".into());
    assert!(state.observe(&wrong, AgentOutputTool::None).is_err());
    assert!(state
        .observe(
            &row(4, AgentLogKind::AgentMessage { content: "".into() }),
            AgentOutputTool::None
        )
        .is_err());
    assert_eq!(state, before);
}

#[test]
fn required_index_fields_do_not_default_on_restart() {
    let original = index();
    let value = serde_json::to_value(&original).unwrap();
    let restored: AgentTurnOutputIndex = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(restored, original);
    for field in [
        "agent_id",
        "turn_sequence",
        "through_sequence",
        "changes",
        "totals",
    ] {
        let mut missing = value.clone();
        missing.as_object_mut().unwrap().remove(field);
        assert!(serde_json::from_value::<AgentTurnOutputIndex>(missing).is_err());
    }
}
