use layer3_adapter::{CheckpointReceipt, ConversationSnapshot, WriterState};
use sha2::{Digest, Sha256};

use super::*;
use crate::sandbox::lifecycle_test_support::{agent, sandbox, NOW};
use crate::sandbox::store::{
    SandboxCheckpointAgentGuard, SandboxCheckpointMutation, SandboxLifecycleAgentMutation,
    SandboxLifecycleAgentScope, SandboxLifecycleCommand, SandboxLifecycleCommandStore,
    SandboxLifecycleExecutionStore, SandboxLifecycleMutation, SandboxMutationGuard,
};
use crate::sandbox::{
    AgentLogKind, AgentStatus, SandboxCheckpointConversation, SandboxCheckpointRef,
    SandboxEventKind, SandboxLifecycleOperation, SandboxPausePhase, SandboxWorkState,
    SandboxWorkStep,
};

const LATER: &str = "2026-09-05T00:31:00Z";
const LEASE_END: &str = "2026-09-05T00:45:00Z";

#[tokio::test]
async fn checkpoint_publication_is_complete_guarded_and_gap_free() {
    let (store, claimed, agents) = pause_fixture().await;
    let checkpoint = checkpoint_for(&agents, 1);
    let mutation = checkpoint_mutation(&claimed, &agents, checkpoint.clone());

    store.publish_sandbox_checkpoint(mutation).await.unwrap();

    let stored = store.get_sandbox(&claimed.id).await.unwrap().unwrap();
    assert_eq!(
        stored.lifecycle.checkpoints.latest,
        Some(checkpoint.clone())
    );
    assert!(matches!(
        stored.lifecycle.operation,
        Some(SandboxLifecycleOperation::Pause {
            phase: SandboxPausePhase::PauseAgents,
            checkpoint_epoch: Some(1),
            ..
        })
    ));
    let events = store.sandbox_events(&stored.id, 0, 10).await.unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(
        events[1].kind,
        SandboxEventKind::CheckpointSealed {
            object_reference: checkpoint.object_reference,
        }
    );
}

#[tokio::test]
async fn checkpoint_publication_rejects_an_incomplete_conversation_set_atomically() {
    let (store, claimed, agents) = pause_fixture().await;
    let checkpoint = checkpoint_for(&agents[..1], 1);
    let mutation = checkpoint_mutation(&claimed, &agents, checkpoint);
    let before = store.get_sandbox(&claimed.id).await.unwrap().unwrap();

    assert_eq!(
        store.publish_sandbox_checkpoint(mutation).await,
        Err(SandboxStoreError::Conflict)
    );
    assert_eq!(store.get_sandbox(&claimed.id).await.unwrap(), Some(before));
    assert_eq!(
        store
            .sandbox_events(&claimed.id, 0, 10)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn lifecycle_checkpoint_accepts_an_exact_empty_live_agent_set() {
    let store = InMemorySandboxStore::default();
    let mut value = sandbox();
    value.revision = 1;
    value.work = SandboxWorkState::new(SandboxWorkStep::None, NOW.into());
    store.create_sandbox(value.clone()).await.unwrap();
    store
        .schedule_sandbox_lifecycle(SandboxLifecycleCommand::pause(
            value.id.clone(),
            value.owner,
            NOW.into(),
            None,
        ))
        .await
        .unwrap();
    let claimed = store
        .claim_sandbox_work(&value.id, "worker", NOW.into(), LEASE_END.into())
        .await
        .unwrap()
        .unwrap();

    store
        .publish_sandbox_checkpoint(checkpoint_mutation(&claimed, &[], checkpoint_for(&[], 1)))
        .await
        .unwrap();

    let stored = store.get_sandbox(&value.id).await.unwrap().unwrap();
    assert_eq!(
        stored.lifecycle.checkpoints.latest.unwrap().conversations,
        []
    );
}

#[tokio::test]
async fn checkpoint_metadata_retains_only_the_latest_two_complete_generations() {
    let store = InMemorySandboxStore::default();
    let mut value = sandbox();
    value.revision = 1;
    value.work = SandboxWorkState::new(SandboxWorkStep::None, NOW.into());
    store.create_sandbox(value.clone()).await.unwrap();
    let mut agents = vec![
        agent("agt_a", "conversation-a", 0),
        agent("agt_b", "conversation-b", 0),
    ];
    for agent in &mut agents {
        agent.revision = 1;
        store.create_agent(agent.clone()).await.unwrap();
    }

    for epoch in 1..=3 {
        let current = store.get_sandbox(&value.id).await.unwrap().unwrap();
        let checkpoint = checkpoint_for(&agents, epoch);
        let mut next = current.clone();
        next.lifecycle
            .checkpoints
            .publish(checkpoint.clone())
            .unwrap();
        next.revision += 1;
        next.updated_at = NOW.into();
        store
            .publish_sandbox_checkpoint(SandboxCheckpointMutation {
                guard: SandboxMutationGuard::capture(&current),
                agents: agents
                    .iter()
                    .map(SandboxCheckpointAgentGuard::capture)
                    .collect(),
                checkpoint,
                sandbox: next,
                appended_at: NOW.into(),
            })
            .await
            .unwrap();
    }

    let state = store.lock();
    assert_eq!(state.checkpoints.len(), 2);
    assert_eq!(
        state
            .checkpoints
            .values()
            .map(|checkpoint| checkpoint.epoch)
            .collect::<std::collections::BTreeSet<_>>(),
        [2, 3].into_iter().collect()
    );
}

#[tokio::test]
async fn checkpoint_rejects_a_completed_agent_change_since_capture() {
    let (store, claimed, agents) = pause_fixture().await;
    let mutation = checkpoint_mutation(&claimed, &agents, checkpoint_for(&agents, 1));
    let mut changed = agents[0].clone();
    changed.revision += 1;
    changed.updated_at = LATER.into();
    store.save_agent(agents[0].revision, changed).await.unwrap();

    assert_eq!(
        store.publish_sandbox_checkpoint(mutation).await,
        Err(SandboxStoreError::Conflict)
    );
    assert_eq!(
        store.get_sandbox(&claimed.id).await.unwrap(),
        Some(claimed.clone())
    );
    assert!(store.lock().checkpoints.is_empty());
    assert_eq!(
        store
            .sandbox_events(&claimed.id, 0, 10)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn checkpoint_rejects_new_log_content_even_without_an_agent_revision_change() {
    let (store, claimed, agents) = pause_fixture().await;
    let mutation = checkpoint_mutation(&claimed, &agents, checkpoint_for(&agents, 1));
    store
        .append_agent_log(
            &agents[0].id,
            LATER.into(),
            AgentLogKind::AgentMessage {
                content: "completed after capture".into(),
            },
        )
        .await
        .unwrap();
    store
        .lock()
        .agents
        .get_mut(&agents[0].id)
        .unwrap()
        .conversation
        .last_seq += 1;

    assert_eq!(
        store.publish_sandbox_checkpoint(mutation).await,
        Err(SandboxStoreError::Conflict)
    );
    assert_eq!(store.get_sandbox(&claimed.id).await.unwrap(), Some(claimed));
    assert!(store.lock().checkpoints.is_empty());
}

#[tokio::test]
async fn checkpoint_rejects_duplicate_or_missing_capture_guards() {
    for duplicate in [false, true] {
        let (store, claimed, agents) = pause_fixture().await;
        let mut mutation = checkpoint_mutation(&claimed, &agents, checkpoint_for(&agents, 1));
        if duplicate {
            mutation.agents.push(mutation.agents[0].clone());
        } else {
            mutation.agents.pop();
        }
        assert_eq!(
            store.publish_sandbox_checkpoint(mutation).await,
            Err(SandboxStoreError::Conflict)
        );
        assert!(store.lock().checkpoints.is_empty());
    }
}

#[tokio::test]
async fn checkpoint_refuses_corrupt_log_evidence_atomically() {
    let (store, claimed, agents) = pause_fixture().await;
    let mutation = checkpoint_mutation(&claimed, &agents, checkpoint_for(&agents, 1));
    store.lock().agent_logs.get_mut(&agents[0].id).unwrap()[0].agent_id = agents[1].id.clone();

    assert_eq!(
        store.publish_sandbox_checkpoint(mutation).await,
        Err(SandboxStoreError::SequenceConflict)
    );
    assert_eq!(store.get_sandbox(&claimed.id).await.unwrap(), Some(claimed));
    assert!(store.lock().checkpoints.is_empty());
}

#[tokio::test]
async fn all_live_agent_transition_and_logs_commit_atomically() {
    let (store, claimed, agents) = pause_fixture().await;
    store
        .publish_sandbox_checkpoint(checkpoint_mutation(
            &claimed,
            &agents,
            checkpoint_for(&agents, 1),
        ))
        .await
        .unwrap();
    let claimed = store
        .claim_sandbox_work(&claimed.id, "worker", LATER.into(), LEASE_END.into())
        .await
        .unwrap()
        .unwrap();
    let mutation = pause_agents_mutation(&claimed, &agents);

    store.commit_sandbox_lifecycle(mutation).await.unwrap();

    let stored = store.get_sandbox(&claimed.id).await.unwrap().unwrap();
    assert!(matches!(
        stored.lifecycle.operation,
        Some(SandboxLifecycleOperation::Pause {
            phase: SandboxPausePhase::TeardownRuntime,
            ..
        })
    ));
    for original in agents {
        let parked = store.get_agent(&original.id).await.unwrap().unwrap();
        assert_eq!(parked.status, AgentStatus::Paused);
        assert_eq!(parked.revision, original.revision + 1);
        assert_eq!(
            parked.conversation.last_seq,
            original.conversation.last_seq + 1
        );
        let log = store.agent_log(&parked.id, 0, 10).await.unwrap();
        assert_eq!(log.last().unwrap().seq, parked.conversation.last_seq);
        assert!(matches!(
            log.last().unwrap().kind,
            AgentLogKind::StatusChanged {
                to: AgentStatus::Paused,
                ..
            }
        ));
    }
}

#[tokio::test]
async fn stale_agent_revision_leaves_every_aggregate_and_event_unchanged() {
    let (store, claimed, agents) = pause_fixture().await;
    store
        .publish_sandbox_checkpoint(checkpoint_mutation(
            &claimed,
            &agents,
            checkpoint_for(&agents, 1),
        ))
        .await
        .unwrap();
    let claimed = store
        .claim_sandbox_work(&claimed.id, "worker", LATER.into(), LEASE_END.into())
        .await
        .unwrap()
        .unwrap();
    let mut mutation = pause_agents_mutation(&claimed, &agents);
    mutation.agents[1].expected_revision += 1;
    let before_sandbox = store.get_sandbox(&claimed.id).await.unwrap().unwrap();
    let before_agents = store
        .sandbox_agents(&claimed.id, &PageRequest::default())
        .await
        .unwrap()
        .items;
    let before_events = store.sandbox_events(&claimed.id, 0, 20).await.unwrap();

    assert_eq!(
        store.commit_sandbox_lifecycle(mutation).await,
        Err(SandboxStoreError::Conflict)
    );
    assert_eq!(
        store.get_sandbox(&claimed.id).await.unwrap(),
        Some(before_sandbox)
    );
    assert_eq!(
        store
            .sandbox_agents(&claimed.id, &PageRequest::default())
            .await
            .unwrap()
            .items,
        before_agents
    );
    assert_eq!(
        store.sandbox_events(&claimed.id, 0, 20).await.unwrap(),
        before_events
    );
}

#[tokio::test]
async fn all_live_scope_rejects_a_partial_agent_set() {
    let (store, claimed, agents) = pause_fixture().await;
    store
        .publish_sandbox_checkpoint(checkpoint_mutation(
            &claimed,
            &agents,
            checkpoint_for(&agents, 1),
        ))
        .await
        .unwrap();
    let claimed = store
        .claim_sandbox_work(&claimed.id, "worker", LATER.into(), LEASE_END.into())
        .await
        .unwrap()
        .unwrap();
    let mut mutation = pause_agents_mutation(&claimed, &agents);
    mutation.agents.pop();

    assert_eq!(
        store.commit_sandbox_lifecycle(mutation).await,
        Err(SandboxStoreError::Conflict)
    );
    for agent in agents {
        assert_eq!(
            store.get_agent(&agent.id).await.unwrap().unwrap().status,
            AgentStatus::Ready
        );
    }
}

#[tokio::test]
async fn lifecycle_phase_rejects_unrelated_sandbox_changes() {
    let (store, claimed, agents) = pause_fixture().await;
    store
        .publish_sandbox_checkpoint(checkpoint_mutation(
            &claimed,
            &agents,
            checkpoint_for(&agents, 1),
        ))
        .await
        .unwrap();
    let claimed = store
        .claim_sandbox_work(&claimed.id, "worker", LATER.into(), LEASE_END.into())
        .await
        .unwrap()
        .unwrap();
    let mut mutation = pause_agents_mutation(&claimed, &agents);
    mutation.sandbox.last_activity_at = LATER.into();

    assert!(matches!(
        store.commit_sandbox_lifecycle(mutation).await,
        Err(SandboxStoreError::InvalidTransaction(_))
    ));
}

#[tokio::test]
async fn sandbox_agent_reads_are_scoped_and_cursor_bounded() {
    let (store, claimed, agents) = pause_fixture().await;
    let mut foreign = agent("agt_foreign", "conversation-foreign", 1);
    foreign.revision = 1;
    foreign.sandbox_id = SandboxId("sbx_foreign".into());
    store.create_agent(foreign).await.unwrap();

    let first = store
        .sandbox_agents(&claimed.id, &PageRequest::new(1, None).unwrap())
        .await
        .unwrap();
    assert_eq!(first.items.len(), 1);
    let second = store
        .sandbox_agents(
            &claimed.id,
            &PageRequest::new(1, first.next.clone()).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(second.items.len(), 1);
    assert_ne!(first.items[0].id, second.items[0].id);
    assert_eq!(
        [first.items[0].id.clone(), second.items[0].id.clone()]
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>(),
        agents
            .into_iter()
            .map(|agent| agent.id)
            .collect::<std::collections::BTreeSet<_>>()
    );
}

async fn pause_fixture() -> (InMemorySandboxStore, Sandbox, Vec<Agent>) {
    let store = InMemorySandboxStore::default();
    let mut value = sandbox();
    value.revision = 1;
    value.work = SandboxWorkState::new(SandboxWorkStep::None, NOW.into());
    store.create_sandbox(value.clone()).await.unwrap();

    let mut agents = vec![
        agent("agt_a", "conversation-a", 1),
        agent("agt_b", "conversation-b", 1),
    ];
    for value in &mut agents {
        value.revision = 1;
        store.create_agent(value.clone()).await.unwrap();
        store
            .append_agent_log(
                &value.id,
                NOW.into(),
                AgentLogKind::AgentMessage {
                    content: "prior output".into(),
                },
            )
            .await
            .unwrap();
    }
    store
        .schedule_sandbox_lifecycle(SandboxLifecycleCommand::pause(
            value.id.clone(),
            value.owner,
            NOW.into(),
            None,
        ))
        .await
        .unwrap();
    let claimed = store
        .claim_sandbox_work(&value.id, "worker", NOW.into(), LEASE_END.into())
        .await
        .unwrap()
        .unwrap();
    (store, claimed, agents)
}

fn checkpoint_for(agents: &[Agent], epoch: u64) -> SandboxCheckpointRef {
    let captures = agents
        .iter()
        .map(|agent| {
            let archive = format!("archive-{}-{epoch}", agent.id).into_bytes();
            SandboxCheckpointConversation::new(
                agent.id.clone(),
                agent.conversation.conversation_id.clone().unwrap(),
                ConversationSnapshot {
                    conversation_id: agent.conversation.conversation_id.clone().unwrap(),
                    archive_sha256: format!("{:x}", Sha256::digest(&archive)),
                    archive,
                },
            )
            .unwrap()
        })
        .collect::<Vec<_>>();
    SandboxCheckpointRef::seal(
        &CheckpointReceipt {
            epoch,
            object_reference: format!("checkpoints/{epoch}"),
            sha256: format!("{epoch:064x}"),
            writers: WriterState::Held,
        },
        &captures,
        NOW,
    )
    .unwrap()
}

fn checkpoint_mutation(
    claimed: &Sandbox,
    agents: &[Agent],
    checkpoint: SandboxCheckpointRef,
) -> SandboxCheckpointMutation {
    let mut next = claimed.clone();
    let lease = claimed.work.lease.as_ref().unwrap();
    next.publish_lifecycle_checkpoint(checkpoint.clone(), &lease.holder, lease.fence, NOW.into())
        .unwrap();
    SandboxCheckpointMutation {
        guard: SandboxMutationGuard::capture(claimed),
        agents: agents
            .iter()
            .map(SandboxCheckpointAgentGuard::capture)
            .collect(),
        checkpoint,
        sandbox: next,
        appended_at: NOW.into(),
    }
}

fn pause_agents_mutation(claimed: &Sandbox, agents: &[Agent]) -> SandboxLifecycleMutation {
    let mut next_sandbox = claimed.clone();
    let lease = claimed.work.lease.as_ref().unwrap();
    next_sandbox
        .complete_lifecycle_agent_pause(&lease.holder, lease.fence, LATER.into())
        .unwrap();

    let mut events = Vec::new();
    let changes = agents
        .iter()
        .map(|current| {
            let mut next = current.clone();
            next.transition(AgentStatus::Paused, LATER.into(), None)
                .unwrap();
            next.conversation.last_seq += 1;
            let log = AgentLogKind::StatusChanged {
                from: current.status,
                to: AgentStatus::Paused,
            };
            events.push(SandboxEventKind::AgentStatusChanged {
                agent_id: current.id.clone(),
                from: current.status,
                to: AgentStatus::Paused,
            });
            SandboxLifecycleAgentMutation {
                expected_revision: current.revision,
                agent: next,
                logs: vec![log],
            }
        })
        .collect();
    events.push(SandboxEventKind::WorkStepCompleted {
        step: SandboxWorkStep::RunLifecycleOperation,
    });
    SandboxLifecycleMutation {
        guard: SandboxMutationGuard::capture(claimed),
        sandbox: next_sandbox,
        agents: changes,
        agent_scope: SandboxLifecycleAgentScope::AllLive,
        appended_at: LATER.into(),
        events,
    }
}
