use super::*;
use crate::sandbox::agent_reads_fold::fold_agent_log;

fn fixture(
    queued: bool,
    gaps: bool,
    active: bool,
) -> (Agent, Vec<AgentLogEntry>, AgentTranscriptWindow) {
    let mut agent = agent("opening", "wt", "ws", "sbx", "owner", AgentStatus::Ready);
    agent.conversation.conversation_id = Some("native".into());
    agent.conversation.runtime_generation = Some(1);
    let mut kinds = vec![
        AgentLogKind::Created {
            workspace_id: agent.workspace_id.clone(),
            worktree_id: agent.worktree_id.clone(),
        },
        AgentLogKind::ConversationOpened {
            conversation_id: "native".into(),
            runtime_generation: 1,
        },
    ];
    if queued {
        kinds.push(AgentLogKind::OwnerMessage {
            content: "question".into(),
        });
    }
    let turn = kinds.len() as u64 + 1;
    kinds.push(AgentLogKind::TurnStarted {
        turn_sequence: turn,
        origin_key: queued.then(|| queued_message_origin(3)),
        model: None,
    });
    if !queued {
        kinds.push(AgentLogKind::OwnerMessage {
            content: "question".into(),
        });
    }
    kinds.push(AgentLogKind::TurnMetadataSet {
        turn_sequence: turn,
        metadata: [("mode".into(), "review".into())].into(),
    });
    kinds.push(AgentLogKind::AgentMessage {
        content: "answer".into(),
    });
    if gaps {
        kinds.push(AgentLogKind::ConversationGap { frames: 2 });
    }
    if !active {
        kinds.push(AgentLogKind::TurnCompleted {
            turn_sequence: turn,
        });
    }
    agent.conversation.active_turn = active.then_some(turn);
    let entries: Vec<_> = kinds
        .into_iter()
        .enumerate()
        .map(|(index, kind)| AgentLogEntry {
            agent_id: agent.id.clone(),
            seq: index as u64 + 1,
            appended_at: NOW.into(),
            kind,
        })
        .collect();
    agent.conversation.last_seq = entries.len() as u64;
    let window = AgentTranscriptWindow {
        start: entries[(turn - 1) as usize].clone(),
        owner: entries
            .iter()
            .find(|row| matches!(row.kind, AgentLogKind::OwnerMessage { .. }))
            .unwrap()
            .clone(),
        metadata: entries
            .iter()
            .find(|row| matches!(row.kind, AgentLogKind::TurnMetadataSet { .. }))
            .cloned(),
        terminal: entries
            .iter()
            .find(|row| matches!(row.kind, AgentLogKind::TurnCompleted { .. }))
            .cloned(),
        gaps: entries
            .iter()
            .filter(|row| matches!(row.kind, AgentLogKind::ConversationGap { .. }))
            .cloned()
            .collect(),
    };
    (agent, entries, window)
}

#[test]
fn opening_window_headers_match_complete_fold_for_queued_active_and_gap_turns() {
    for queued in [false, true] {
        for gaps in [false, true] {
            for active in [false, true] {
                let (agent, entries, window) = fixture(queued, gaps, active);
                let mut expected = fold_agent_log(&agent, &entries)
                    .unwrap()
                    .transcript
                    .turns
                    .remove(0);
                expected
                    .messages
                    .retain(|row| row.sequence < expected.turn_sequence);
                expected.agent_messages.clear();
                assert_eq!(window.header(&agent).unwrap(), expected);
            }
        }
    }
}

#[test]
fn opening_window_rejects_changed_identity_boundary_and_evidence() {
    let (mut agent, _, mut window) = fixture(false, true, false);
    agent.id = AgentId("other".into());
    assert!(window.header(&agent).is_err());
    agent.id = window.start.agent_id.clone();
    window.gaps[0].seq = window.start.seq;
    assert!(window.header(&agent).is_err());
    window.gaps.clear();
    window.metadata.as_mut().unwrap().kind = AgentLogKind::ModeChanged {
        mode: "idle".into(),
    };
    assert!(window.header(&agent).is_err());
    window.metadata = None;
    window.terminal = None;
    assert!(window.header(&agent).is_err());
}

#[test]
fn opening_usage_summary_matches_full_fold_across_generations_and_later_raw_usage() {
    let (mut agent, mut entries, _) = fixture(false, false, false);
    let mut summary = None;
    for kind in [
        AgentLogKind::Usage {
            input_tokens: 9,
            output_tokens: 3,
            context_used: 2,
            context_window: Some(100),
        },
        AgentLogKind::ConversationOpened {
            conversation_id: "next".into(),
            runtime_generation: 2,
        },
        AgentLogKind::UsageUpdated {
            input_tokens: 20,
            output_tokens: 7,
            context_used: 6,
            context_window: Some(200),
            turn_sequence: Some(3),
            turn_input_tokens: 11,
            turn_output_tokens: 4,
        },
        AgentLogKind::ConversationOpened {
            conversation_id: "after-summary".into(),
            runtime_generation: 3,
        },
        AgentLogKind::Usage {
            input_tokens: 99,
            output_tokens: 99,
            context_used: 99,
            context_window: Some(1000),
        },
    ] {
        let row = AgentLogEntry {
            agent_id: agent.id.clone(),
            seq: entries.len() as u64 + 1,
            appended_at: NOW.into(),
            kind,
        };
        if matches!(row.kind, AgentLogKind::UsageUpdated { .. }) {
            summary = Some(row.clone());
        }
        entries.push(row);
    }
    agent.conversation.conversation_id = Some("after-summary".into());
    agent.conversation.runtime_generation = Some(3);
    agent.conversation.last_seq = entries.len() as u64;
    let summary = summary.unwrap();
    assert_eq!(
        AgentUsageReading::from_summary(&agent, &summary).unwrap(),
        fold_agent_log(&agent, &entries).unwrap().usage
    );
    assert!(AgentUsageReading::from_summary(&agent, entries.last().unwrap()).is_err());
    agent.conversation.last_seq = summary.seq - 1;
    assert!(AgentUsageReading::from_summary(&agent, &summary).is_err());
}
