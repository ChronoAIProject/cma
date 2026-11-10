use super::*;
use crate::store::sandbox_maintenance::mongo_integration_tests::{active_sandbox, ready_agent};
use layer2_core::sandbox::{
    AgentLogKind::*, AgentOperationDelivery, AgentOperationKind, AgentStatus, QueuedAgentMessage,
};

const AT: &str = "2026-09-05T00:00:00Z";

fn apply_agent(agent: &mut Agent, entry: &AgentLogEntry) {
    agent.conversation.last_seq = entry.seq;
    match &entry.kind {
        ConversationOpened {
            conversation_id,
            runtime_generation,
        } => {
            agent.conversation.conversation_id = Some(conversation_id.clone());
            agent.conversation.runtime_generation = Some(*runtime_generation);
        }
        TurnStarted {
            turn_sequence,
            origin_key,
            ..
        } => {
            agent.conversation.active_turn = Some(*turn_sequence);
            agent.status = AgentStatus::Working;
            if origin_key.is_some() {
                agent.conversation.queued_messages.clear();
            }
        }
        OwnerMessage { content } if agent.conversation.active_turn.is_none() => {
            agent
                .conversation
                .queued_messages
                .push(QueuedAgentMessage::new(entry.seq, content.clone(), AT.into()).unwrap());
        }
        TurnCompleted { .. } | TurnFailed { .. } | TurnInterrupted { .. } => {
            agent.conversation.active_turn = None;
            agent.status = AgentStatus::Ready;
        }
        RuntimeGenerationRetired {
            runtime_generation, ..
        } => {
            agent.conversation.runtime_generation = Some(*runtime_generation);
        }
        _ => {}
    }
}

#[test]
fn every_restart_partition_matches_full_domain_fold() {
    let parent = active_sandbox("sbx_checkpoint");
    let mut initial = ready_agent(&parent, "agt_checkpoint");
    initial.conversation.last_seq = 0;
    initial.conversation.conversation_id = None;
    initial.conversation.runtime_generation = None;
    for (variant, tail) in [
        vec![
            AgentMessage {
                content: "answer".into(),
            },
            TurnCompleted { turn_sequence: 3 },
        ],
        vec![
            AgentMessage {
                content: "answer".into(),
            },
            AgentMessage {
                content: "  ".into(),
            },
            TurnCompleted { turn_sequence: 3 },
        ],
        vec![
            AgentMessage {
                content: "answer".into(),
            },
            TurnInterrupted { turn_sequence: 3 },
        ],
        vec![
            AgentMessage {
                content: "answer".into(),
            },
            TurnFailed {
                turn_sequence: 3,
                failure: layer2_core::sandbox::TypedFailure {
                    class: "test".into(),
                    detail: "test".into(),
                },
            },
        ],
        vec![
            ConversationGap { frames: 1 },
            AgentMessage {
                content: "answer".into(),
            },
            TurnCompleted { turn_sequence: 3 },
        ],
        vec![
            AgentMessage {
                content: "answer".into(),
            },
            OperationDeliveryChanged {
                operation_sequence: 6,
                operation: AgentOperationKind::Steer,
                state: AgentOperationDelivery::DispatchFenced,
            },
            OwnerMessage {
                content: "steer".into(),
            },
            TurnCompleted { turn_sequence: 3 },
        ],
        vec![
            AgentMessage {
                content: "answer".into(),
            },
            TurnCompleted { turn_sequence: 3 },
            OwnerMessage {
                content: "queued".into(),
            },
        ],
        vec![
            AgentMessage {
                content: "answer".into(),
            },
            TurnCompleted { turn_sequence: 3 },
            OwnerMessage {
                content: "queued".into(),
            },
            TurnStarted {
                turn_sequence: 8,
                origin_key: Some("queued-message:7".into()),
                model: None,
            },
            AgentMessage {
                content: "queued answer".into(),
            },
            TurnCompleted { turn_sequence: 8 },
        ],
        vec![
            ConversationGap { frames: 1 },
            AgentMessage {
                content: "incomplete".into(),
            },
            TurnCompleted { turn_sequence: 3 },
            TurnStarted {
                turn_sequence: 8,
                origin_key: None,
                model: None,
            },
            OwnerMessage {
                content: "new question".into(),
            },
            AgentMessage {
                content: "clean answer".into(),
            },
            TurnCompleted { turn_sequence: 8 },
        ],
        vec![
            TurnCompleted { turn_sequence: 3 },
            ConversationGap { frames: 2 },
            ConversationOpened {
                conversation_id: "next".into(),
                runtime_generation: 2,
            },
            TurnStarted {
                turn_sequence: 8,
                origin_key: None,
                model: None,
            },
            OwnerMessage {
                content: "new".into(),
            },
            AgentMessage {
                content: "clean".into(),
            },
            TurnCompleted { turn_sequence: 8 },
        ],
    ]
    .into_iter()
    .enumerate()
    {
        let mut kinds = vec![
            Created {
                workspace_id: initial.workspace_id.clone(),
                worktree_id: initial.worktree_id.clone(),
            },
            ConversationOpened {
                conversation_id: "conversation".into(),
                runtime_generation: 1,
            },
            TurnStarted {
                turn_sequence: 3,
                model: None,
                origin_key: None,
            },
            OwnerMessage {
                content: "question".into(),
            },
        ];
        kinds.extend(tail);
        let entries = kinds
            .into_iter()
            .enumerate()
            .map(|(i, kind)| AgentLogEntry {
                agent_id: initial.id.clone(),
                seq: i as u64 + 1,
                appended_at: AT.into(),
                kind,
            })
            .collect::<Vec<_>>();
        for cut in 0..=entries.len() {
            let mut agent = initial.clone();
            let mut checkpoint = SemanticCheckpoint::empty();
            for entry in &entries[..cut] {
                apply_agent(&mut agent, entry);
                checkpoint = checkpoint
                    .after_append(std::slice::from_ref(entry), &agent)
                    .unwrap();
            }
            checkpoint = metadata::parse_semantic(
                &mongodb::bson::serialize_to_document(&checkpoint).unwrap(),
            )
            .unwrap();
            for entry in &entries[cut..] {
                apply_agent(&mut agent, entry);
                checkpoint = checkpoint
                    .after_append(std::slice::from_ref(entry), &agent)
                    .unwrap();
            }
            assert!(checkpoint.valid, "variant {variant} cut {cut}");
            assert_eq!(
                checkpoint.answer_context(),
                layer2_core::sandbox::agent_io::answer_context_for_log(&agent, &entries).unwrap(),
                "variant {variant} cut {cut}"
            );
            let rebuilt = SemanticCheckpoint::from_entries(&agent, &entries).unwrap();
            assert_eq!(checkpoint.latest_turn(), rebuilt.latest_turn());
        }
    }
}
