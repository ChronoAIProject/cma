use super::*;
use crate::store::agent_log_integrity::INTEGRITY_FIELD;
use crate::store::agents::agent_replacement_fields;

pub(super) async fn prepare_history(
    store: MongoSandboxStore,
    mut agent: Agent,
    count: u64,
) -> (MongoSandboxStore, Agent) {
    assert!(count >= 6);
    // Replace the legacy test helper's deliberately incomplete first event.
    store
        .agent_log
        .delete_many(doc! { "agent_id": agent.id.as_str() })
        .await
        .unwrap();
    let mut kinds = vec![
        AgentLogKind::Created {
            workspace_id: agent.workspace_id.clone(),
            worktree_id: agent.creation_worktree_id().clone(),
        },
        AgentLogKind::ConversationOpened {
            conversation_id: agent.conversation.conversation_id.clone().unwrap(),
            runtime_generation: 1,
        },
    ];
    kinds.resize(
        count as usize - 4,
        AgentLogKind::WorkStepCompleted {
            step: AgentWorkStep::None,
        },
    );
    kinds.extend([
        AgentLogKind::TurnStarted {
            turn_sequence: count - 3,
            origin_key: None,
            model: None,
        },
        AgentLogKind::OwnerMessage {
            content: "question".into(),
        },
        AgentLogKind::AgentMessage {
            content: "answer".into(),
        },
        AgentLogKind::TurnCompleted {
            turn_sequence: count - 3,
        },
    ]);
    let rows = kinds
        .into_iter()
        .enumerate()
        .map(|(index, kind)| {
            log_document(&AgentLogEntry {
                agent_id: agent.id.clone(),
                seq: index as u64 + 1,
                appended_at: NOW.into(),
                kind,
            })
            .unwrap()
        })
        .collect::<Vec<_>>();
    store.agent_log.insert_many(rows).await.unwrap();
    agent.conversation.last_seq = count;
    agent.revision += 1;
    let mut fields = agent_replacement_fields(&agent).unwrap();
    fields.insert("last_seq", count as i64);
    store
        .agents
        .update_one(
            doc! { "_id": agent.id.as_str() },
            // This fixture replaces the legacy history before initialization.
            // The old helper's tail hash cannot describe the replacement rows.
            doc! { "$set": fields, "$unset": { INTEGRITY_FIELD: "" } },
        )
        .await
        .unwrap();
    let store = store
        .prepare_bounded_admission(std::slice::from_ref(&agent.id))
        .await
        .unwrap();
    (store, agent)
}
