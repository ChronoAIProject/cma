use crate::sandbox::store::{
    AgentAdmissionStore, AgentConversationMutation, AgentConversationStore, AgentStore,
    AgentWorkMutation, AgentWorkerStore, VersionedWorktree,
};
use crate::sandbox::{AgentLogKind, AgentStatus, AgentWorkStep, WorktreeStatus};

use super::agent_admission_tests::*;

async fn ready_agent() -> (super::InMemorySandboxStore, crate::sandbox::Agent) {
    let store = ready_store(1).await;
    let creation = created(store.admit_agent(admission("conversation")).await.unwrap());
    let claimed = store
        .claim_agent_work(
            &creation.agent.id,
            "worker",
            NOW.into(),
            "2026-09-05T00:01:00Z".into(),
        )
        .await
        .unwrap()
        .unwrap();
    let fence = claimed.work.lease.as_ref().unwrap().fence;
    let mut agent = claimed.clone();
    agent.conversation.conversation_id = Some("conversation-1".into());
    agent.conversation.runtime_generation = Some(1);
    agent.conversation.last_seq = 3;
    agent
        .work
        .advance("worker", fence, AgentWorkStep::None, NOW.into())
        .unwrap();
    agent
        .transition(AgentStatus::Ready, NOW.into(), None)
        .unwrap();
    let mut worktree = creation.worktree;
    let expected_worktree_revision = worktree.revision;
    worktree.transition(WorktreeStatus::Ready).unwrap();
    store
        .commit_agent_work(AgentWorkMutation {
            expected_agent_revision: claimed.revision,
            holder: "worker".into(),
            fence,
            agent: agent.clone(),
            worktree: Some(VersionedWorktree {
                expected_revision: expected_worktree_revision,
                worktree,
            }),
            appended_at: NOW.into(),
            sandbox_event: None,
            agent_logs: vec![
                AgentLogKind::ConversationOpened {
                    conversation_id: "conversation-1".into(),
                    runtime_generation: 1,
                },
                AgentLogKind::StatusChanged {
                    from: AgentStatus::Starting,
                    to: AgentStatus::Ready,
                },
            ],
        })
        .await
        .unwrap();
    (store, agent)
}

fn begin_turn(mut agent: crate::sandbox::Agent) -> AgentConversationMutation {
    let expected_agent_revision = agent.revision;
    let expected_last_seq = agent.conversation.last_seq;
    let turn_sequence = expected_last_seq + 1;
    agent
        .transition(AgentStatus::Working, NOW.into(), None)
        .unwrap();
    agent.conversation.active_turn = Some(turn_sequence);
    agent.conversation.last_seq += 2;
    AgentConversationMutation {
        expected_agent_revision,
        expected_last_seq,
        runtime_generation: 1,
        agent,
        appended_at: NOW.into(),
        agent_logs: vec![
            AgentLogKind::TurnStarted {
                turn_sequence,
                origin_key: None,
            },
            AgentLogKind::OwnerMessage {
                content: "Inspect the proof.".into(),
            },
        ],
    }
}

#[tokio::test]
async fn conversation_mutation_updates_agent_and_log_atomically() {
    let (store, agent) = ready_agent().await;
    let mutation = begin_turn(agent);
    let expected = mutation.agent.clone();

    let appended = store.commit_agent_conversation(mutation).await.unwrap();

    assert_eq!(
        appended.iter().map(|entry| entry.seq).collect::<Vec<_>>(),
        [4, 5]
    );
    assert_eq!(store.get_agent(&expected.id).await.unwrap(), Some(expected));
}

#[tokio::test]
async fn stale_runtime_generation_cannot_write() {
    let (store, agent) = ready_agent().await;
    let mut mutation = begin_turn(agent.clone());
    mutation.runtime_generation = 2;

    assert_eq!(
        store.commit_agent_conversation(mutation).await.unwrap_err(),
        crate::sandbox::store::SandboxStoreError::Conflict
    );
    assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(agent));
}

#[tokio::test]
async fn late_sequence_failure_rolls_back_the_agent() {
    let (store, agent) = ready_agent().await;
    let mut mutation = begin_turn(agent.clone());
    mutation.agent.conversation.last_seq += 1;

    assert_eq!(
        store.commit_agent_conversation(mutation).await.unwrap_err(),
        crate::sandbox::store::SandboxStoreError::Conflict
    );
    assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(agent));
}
