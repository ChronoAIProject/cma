use super::*;
use std::collections::BTreeMap;

fn start() -> (Head, Option<AgentTurnOutputIndex>) {
    let (agent, mut head, _) = super::super::super::writer_projection::tests::opened();
    head.last_seq = 20;
    head.projection_through = 20;
    let mut index = None;
    let entry = AgentLogEntry {
        agent_id: agent.id,
        seq: 3,
        appended_at: "2026-11-11T00:00:00Z".into(),
        kind: AgentLogKind::TurnStarted {
            turn_sequence: 3,
            origin_key: None,
            model: Some("fixture".into()),
        },
    };
    fold(&head, &mut index, &entry, AgentOutputTool::None).unwrap();
    (head, index)
}

fn event(index: &Option<AgentTurnOutputIndex>, kind: AgentLogKind) -> AgentLogEntry {
    let index = index.as_ref().unwrap();
    AgentLogEntry {
        agent_id: index.agent_id.clone(),
        seq: index.through_sequence + 1,
        appended_at: "2026-11-11T00:00:00Z".into(),
        kind,
    }
}

#[test]
fn output_envelopes_keep_numeric_seek_keys_and_reject_corrupt_counters() {
    let (head, mut index) = start();
    let entry = event(
        &index,
        AgentLogKind::AgentMessage {
            content: " ".into(),
        },
    );
    let rows = fold(&head, &mut index, &entry, AgentOutputTool::None).unwrap();
    let row = rows
        .iter()
        .find(|r| r.get_str("kind").unwrap() == "output_change")
        .unwrap();
    assert_eq!(row.get_i64("turn_sequence").unwrap(), 3);
    assert_eq!(row.get_i64("change_ordinal").unwrap(), 0);
    let EvidenceRecord::OutputChange { change } =
        read_evidence(&head, "output_change", "3:0", row.clone()).unwrap()
    else {
        panic!("change")
    };
    assert_eq!(change.after.nonempty_messages, 1);
    for field in ["turn_sequence", "change_ordinal"] {
        let mut corrupt_row = row.clone();
        corrupt_row.insert(field, 99_i64);
        assert!(read_evidence(&head, "output_change", "3:0", corrupt_row).is_err());
        let mut wrong_type = row.clone();
        wrong_type.insert(field, 0_i32);
        assert!(read_evidence(&head, "output_change", "3:0", wrong_type).is_err());
        let mut missing = row.clone();
        missing.remove(field);
        assert!(read_evidence(&head, "output_change", "3:0", missing).is_err());
    }
    let mut bad = change;
    bad.after.messages += 1;
    assert!(document(&head, EvidenceRecord::OutputChange { change: bad }).is_err());
    let mut turn = rows[0].clone();
    turn.insert("change_ordinal", 0_i64);
    assert!(read_evidence(&head, "turn_output", "3", turn).is_err());
}

#[test]
fn first_terminal_evidence_stays_immutable_while_late_tool_output_advances() {
    let (head, mut index) = start();
    let mut tool = None;
    let mut changes = BTreeMap::new();
    let call_id = "call:fixture".to_owned();
    for kind in [
        AgentLogKind::ToolCallStarted {
            call_id: call_id.clone(),
            title: "tool".into(),
            kind: "shell".into(),
            status: "running".into(),
        },
        AgentLogKind::ToolOutputReported {
            call_id: call_id.clone(),
            output: layer2_core::sandbox::RecordedToolOutput {
                text: "first".into(),
                truncated: false,
            },
        },
        AgentLogKind::ToolCallUpdated {
            call_id: call_id.clone(),
            status: "completed".into(),
            title: None,
            output: None,
        },
        AgentLogKind::ToolOutputReported {
            call_id: call_id.clone(),
            output: layer2_core::sandbox::RecordedToolOutput {
                text: "late".into(),
                truncated: false,
            },
        },
        AgentLogKind::ToolCallStarted {
            call_id: call_id.clone(),
            title: "duplicate".into(),
            kind: "shell".into(),
            status: "failed".into(),
        },
    ] {
        let entry = event(&index, kind);
        let rows = fold(
            &head,
            &mut index,
            &entry,
            tool.as_ref()
                .map_or(AgentOutputTool::Absent, AgentOutputTool::Stored),
        )
        .unwrap();
        for row in rows {
            let kind = row.get_str("kind").unwrap();
            let key = row.get_str("entity_key").unwrap();
            match read_evidence(&head, kind, key, row.clone()).unwrap() {
                EvidenceRecord::ToolOutput { tool: next } => tool = Some(next),
                EvidenceRecord::OutputChange { change } => {
                    assert!(changes.insert(change.ordinal, change).is_none());
                }
                _ => {}
            }
        }
    }
    assert_eq!(changes.len(), 2);
    let terminal = changes[&1].tool.as_ref().unwrap();
    assert_eq!(terminal.start_sequence, 4);
    assert_eq!(terminal.output_sequence, Some(5));
    assert_eq!(terminal.terminal_sequence, Some(6));
    assert_eq!(tool.as_ref().unwrap().output_sequence, Some(7));
    assert_eq!(index.as_ref().unwrap().changes, 2);
    assert_eq!(index.as_ref().unwrap().totals.tool_starts, 1);
    assert_eq!(index.as_ref().unwrap().totals.tool_finishes, 1);
    assert_ne!(tool_key(3, "4:call"), tool_key(34, "call"));
}

#[test]
fn new_turn_resets_counters_and_old_tool_state_cannot_cross_turns() {
    let (head, mut index) = start();
    let entry = event(&index, AgentLogKind::AgentMessage { content: "".into() });
    fold(&head, &mut index, &entry, AgentOutputTool::None).unwrap();
    let entry = event(
        &index,
        AgentLogKind::TurnStarted {
            turn_sequence: 5,
            origin_key: None,
            model: None,
        },
    );
    fold(&head, &mut index, &entry, AgentOutputTool::None).unwrap();
    assert_eq!(index.as_ref().unwrap().turn_sequence, 5);
    assert_eq!(index.as_ref().unwrap().totals.items, 0);
    let old = AgentToolOutputState {
        call_id: "x".into(),
        turn_sequence: 3,
        item_ordinal: 0,
        start_sequence: 4,
        output_sequence: None,
        terminal_sequence: None,
    };
    let entry = event(
        &index,
        AgentLogKind::ToolOutputReported {
            call_id: "x".into(),
            output: layer2_core::sandbox::RecordedToolOutput {
                text: "late".into(),
                truncated: false,
            },
        },
    );
    let before = index.clone();
    assert!(fold(&head, &mut index, &entry, AgentOutputTool::Stored(&old)).is_err());
    assert_eq!(index, before);
}
