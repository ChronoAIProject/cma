use super::*;
use crate::ids::AgentId;

fn semantic_entry(seq: u64, kind: crate::sandbox::AgentLogKind) -> AgentLogEntry {
    AgentLogEntry {
        agent_id: AgentId("agt_semantic".into()),
        seq,
        appended_at: "2026-09-05T00:00:00Z".into(),
        kind,
    }
}

#[test]
fn semantic_checkpoint_uses_the_latest_message_even_when_whitespace() {
    use crate::sandbox::AgentLogKind;

    let mut checkpoint = AgentLogCheckpoint::empty();
    for (seq, kind) in [
        (
            1,
            AgentLogKind::ConversationOpened {
                conversation_id: "conversation".into(),
                runtime_generation: 1,
            },
        ),
        (
            2,
            AgentLogKind::TurnStarted {
                turn_sequence: 2,
                model: None,
                origin_key: None,
            },
        ),
        (
            3,
            AgentLogKind::OwnerMessage {
                content: "question".into(),
            },
        ),
        (
            4,
            AgentLogKind::AgentMessage {
                content: "answer".into(),
            },
        ),
        (
            5,
            AgentLogKind::AgentMessage {
                content: "   ".into(),
            },
        ),
        (6, AgentLogKind::TurnCompleted { turn_sequence: 2 }),
    ] {
        checkpoint.apply_entry(&semantic_entry(seq, kind)).unwrap();
    }
    assert_eq!(checkpoint.answer_context(), None);
}

#[test]
fn semantic_checkpoint_rejects_a_nonadjacent_steering_owner_message() {
    use crate::sandbox::{AgentLogKind, AgentOperationDelivery, AgentOperationKind};

    let mut checkpoint = AgentLogCheckpoint::empty();
    for (seq, kind) in [
        (
            1,
            AgentLogKind::ConversationOpened {
                conversation_id: "conversation".into(),
                runtime_generation: 1,
            },
        ),
        (
            2,
            AgentLogKind::TurnStarted {
                turn_sequence: 2,
                model: None,
                origin_key: None,
            },
        ),
        (
            3,
            AgentLogKind::OwnerMessage {
                content: "question".into(),
            },
        ),
        (
            4,
            AgentLogKind::OperationDeliveryChanged {
                operation_sequence: 4,
                operation: AgentOperationKind::Steer,
                state: AgentOperationDelivery::DispatchFenced,
            },
        ),
    ] {
        checkpoint.apply_entry(&semantic_entry(seq, kind)).unwrap();
    }
    assert!(checkpoint
        .apply_entry(&semantic_entry(
            5,
            AgentLogKind::AgentMessage {
                content: "wrong event".into(),
            },
        ))
        .is_err());
}

#[test]
fn semantic_checkpoint_rejects_a_steering_timestamp_mismatch() {
    use crate::sandbox::{AgentLogKind, AgentOperationDelivery, AgentOperationKind};

    let mut checkpoint = AgentLogCheckpoint::empty();
    for (seq, kind) in [
        (
            1,
            AgentLogKind::ConversationOpened {
                conversation_id: "conversation".into(),
                runtime_generation: 1,
            },
        ),
        (
            2,
            AgentLogKind::TurnStarted {
                turn_sequence: 2,
                model: None,
                origin_key: None,
            },
        ),
        (
            3,
            AgentLogKind::OwnerMessage {
                content: "question".into(),
            },
        ),
        (
            4,
            AgentLogKind::OperationDeliveryChanged {
                operation_sequence: 4,
                operation: AgentOperationKind::Steer,
                state: AgentOperationDelivery::DispatchFenced,
            },
        ),
    ] {
        checkpoint.apply_entry(&semantic_entry(seq, kind)).unwrap();
    }
    let mut owner = semantic_entry(
        5,
        AgentLogKind::OwnerMessage {
            content: "steering input".into(),
        },
    );
    owner.appended_at = "2026-09-05T00:00:01Z".into();
    assert!(checkpoint.apply_entry(&owner).is_err());
}

#[test]
fn semantic_checkpoint_rejects_duplicate_creation() {
    use crate::sandbox::AgentLogKind;

    let mut checkpoint = AgentLogCheckpoint::empty();
    checkpoint
        .apply_entry(&semantic_entry(
            1,
            AgentLogKind::Created {
                workspace_id: crate::ids::WorkspaceId("workspace".into()),
                worktree_id: crate::ids::WorktreeId("worktree".into()),
            },
        ))
        .unwrap();
    assert!(checkpoint
        .apply_entry(&semantic_entry(
            2,
            AgentLogKind::Created {
                workspace_id: crate::ids::WorkspaceId("workspace".into()),
                worktree_id: crate::ids::WorktreeId("worktree".into()),
            }
        ))
        .is_err());
}
