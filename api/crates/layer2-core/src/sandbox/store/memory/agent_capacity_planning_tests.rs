use super::*;
use crate::sandbox::store::{AgentConversationCapacitySettlement, AgentMessageCheckpoint};

#[tokio::test]
async fn hundred_entry_terminal_batch_becomes_bounded_capacity_failure_without_publication() {
    let (_, mut current, _) = working_agent().await;
    current.conversation.last_seq = 9_990;
    let mut next = current.clone();
    next.transition(AgentStatus::Ready, NOW.into(), None)
        .unwrap();
    next.conversation.active_turn = None;
    next.conversation.last_seq += 100;
    let mut kinds = vec![
        AgentLogKind::AgentMessage {
            content: "final batch".into()
        };
        98
    ];
    kinds.push(AgentLogKind::TurnCompleted {
        turn_sequence: current.conversation.active_turn.unwrap(),
    });
    kinds.push(AgentLogKind::StatusChanged {
        from: AgentStatus::Working,
        to: AgentStatus::Ready,
    });
    let entries = kinds
        .iter()
        .enumerate()
        .map(|(i, kind)| AgentLogEntry {
            agent_id: current.id.clone(),
            seq: 9_991 + i as u64,
            appended_at: NOW.into(),
            kind: kind.clone(),
        })
        .collect();
    let checkpoint = AgentMessageCheckpoint {
        version: 1,
        update_id: uuid::Uuid::new_v4().to_string(),
        update_fingerprint: "a".repeat(64),
        native_conversation_id: current.conversation.conversation_id.clone().unwrap(),
        runtime_generation: 1,
        active_turn: None,
        after_seq: 10_090,
        assembly: AgentMessageAssembly::default(),
        outcome: crate::sandbox::AgentUpdateOutcome {
            appended: entries,
            turn_closed: true,
        },
        output: None,
    };
    let mutation = AgentConversationMutation {
        settings_fence: None,
        command_claim: None,
        execution_fence: None,
        trigger_execution_fence: None,
        admission_guard: None,
        expected_agent_revision: current.revision,
        expected_last_seq: current.conversation.last_seq,
        runtime_generation: 1,
        agent: next,
        appended_at: NOW.into(),
        agent_logs: kinds,
    };
    let plan = AgentConversationCapacitySettlement::new(current, None, checkpoint, mutation, true)
        .unwrap();
    plan.checkpoint().validate().unwrap();
    assert!(plan.compact_failure().unwrap().is_none());
    assert_eq!(plan.mutation().agent_logs.len(), 2);
    assert_eq!(plan.mutation().agent.conversation.last_seq, 9_992);
    assert!(
        matches!(&plan.mutation().agent_logs[0], AgentLogKind::TurnFailed { failure, .. } if failure.class == "agent_log_capacity_exceeded")
    );
    assert_eq!(plan.refused().len(), 100);
}
