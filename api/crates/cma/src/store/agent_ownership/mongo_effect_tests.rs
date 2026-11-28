use super::*;
use crate::store::sandbox_maintenance::mongo_integration_tests::{database, fixture};
use crate::store::MongoSandboxStore;
use layer2_core::agent_ownership::{Authority, OwnershipCoordinator};
use layer2_core::sandbox::store::{
    AgentConversationMutation, AgentConversationStore, AgentMessageCheckpoint, AgentStore,
    AgentWorkerStore, SandboxStoreError,
};
use layer2_core::sandbox::{
    Agent, AgentLogKind, AgentMessageAssembly, AgentUpdateOutcome, AgentWorkStep,
};
use std::sync::Arc;

const NOW: &str = "2026-11-05T00:00:00Z";
const LONG_LEASE: &str = "2026-11-05T00:18:00Z";

async fn members(store: &MongoSandboxStore, count: usize) -> Vec<Arc<OwnershipCoordinator>> {
    let ownership = store.agent_ownership.as_ref().unwrap();
    let mut coordinators = Vec::new();
    for index in 0..count {
        coordinators.push(
            OwnershipCoordinator::register(ownership.clone(), member(&format!("member-{index}")))
                .await
                .unwrap(),
        );
    }
    coordinators
}

async fn acquire(
    store: &MongoSandboxStore,
    coordinators: &[Arc<OwnershipCoordinator>],
    agent: &Agent,
) -> Arc<Authority> {
    let preferred = preferred(store.agent_ownership.as_ref().unwrap(), &agent.id).await;
    coordinators
        .iter()
        .find(|coordinator| coordinator.instance_id == preferred)
        .unwrap()
        .acquire(&agent.id)
        .await
        .unwrap()
        .unwrap()
}

fn mutation(agent: &Agent) -> AgentConversationMutation {
    let mut next = agent.clone();
    next.revision += 1;
    next.conversation.last_seq += 1;
    next.updated_at = NOW.into();
    AgentConversationMutation {
        settings_fence: None,
        command_claim: None,
        execution_fence: None,
        trigger_execution_fence: None,
        admission_guard: None,
        expected_agent_revision: agent.revision,
        expected_last_seq: agent.conversation.last_seq,
        runtime_generation: agent.conversation.runtime_generation.unwrap(),
        agent: next,
        appended_at: NOW.into(),
        agent_logs: vec![AgentLogKind::AgentMessage {
            content: "fixture".into(),
        }],
    }
}

fn checkpoint(agent: &Agent) -> AgentMessageCheckpoint {
    AgentMessageCheckpoint {
        version: 1,
        update_id: uuid::Uuid::new_v4().to_string(),
        update_fingerprint: "a".repeat(64),
        output: None,
        native_conversation_id: agent.conversation.conversation_id.clone().unwrap(),
        runtime_generation: agent.conversation.runtime_generation.unwrap(),
        active_turn: agent.conversation.active_turn,
        after_seq: agent.conversation.last_seq,
        assembly: AgentMessageAssembly::default(),
        outcome: AgentUpdateOutcome {
            appended: vec![],
            turn_closed: false,
        },
    }
}

#[tokio::test]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn actual_conversation_and_checkpoint_writes_reject_missing_and_stale_owner_epochs() {
    for count in [2, 3] {
        let (client, name, off) = database().await;
        let (_, agent) = fixture(&off, "ownership-effects").await;
        let store = off.enable_agent_ownership("member-0".into()).await.unwrap();
        let coordinators = members(&store, count).await;
        let first = acquire(&store, &coordinators, &agent).await;
        assert!(matches!(
            store.commit_agent_conversation(mutation(&agent)).await,
            Err(SandboxStoreError::Conflict)
        ));
        let initial_checkpoint = checkpoint(&agent);
        first
            .run(store.commit_agent_message_update(
                agent.clone(),
                None,
                initial_checkpoint.clone(),
                None,
            ))
            .await
            .unwrap()
            .unwrap();
        let ownership = store.agent_ownership.as_ref().unwrap();
        ownership
            .heartbeat(&first.token.instance_id, true)
            .await
            .unwrap();
        expire(ownership, &first.token).await;
        let successor = acquire(&store, &coordinators, &agent).await;
        assert!(successor.token.epoch > first.token.epoch);
        let next_checkpoint = checkpoint(&agent);
        assert_ne!(next_checkpoint.update_id, initial_checkpoint.update_id);
        let before = store.agent_message_snapshot(&agent.id).await.unwrap();
        assert!(matches!(
            first
                .run(store.commit_agent_conversation(mutation(&agent)))
                .await
                .unwrap(),
            Err(SandboxStoreError::Conflict)
        ));
        assert!(matches!(
            first
                .run(store.commit_agent_message_update(
                    agent.clone(),
                    Some(initial_checkpoint.clone()),
                    next_checkpoint.clone(),
                    None
                ))
                .await
                .unwrap(),
            Err(SandboxStoreError::Conflict)
        ));
        assert_eq!(
            store.agent_message_snapshot(&agent.id).await.unwrap(),
            before
        );
        successor
            .run(store.commit_agent_message_update(
                agent.clone(),
                Some(initial_checkpoint),
                next_checkpoint.clone(),
                None,
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            store.agent_message_checkpoint(&agent.id).await.unwrap(),
            Some(next_checkpoint)
        );
        successor
            .run(store.commit_agent_conversation(mutation(&agent)))
            .await
            .unwrap()
            .unwrap();
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn takeover_discovers_and_reclaims_old_lifecycle_work_without_waiting_eighteen_minutes() {
    let (client, name, off) = database().await;
    let (_, mut agent) = fixture(&off, "ownership-work").await;
    let revision = agent.revision;
    agent
        .work
        .schedule(AgentWorkStep::PublishConnection, NOW.into());
    agent.revision += 1;
    off.save_agent(revision, agent.clone()).await.unwrap();
    let mut store = off.enable_agent_ownership("member-0".into()).await.unwrap();
    let coordinators = members(&store, 3).await;
    let first = acquire(&store, &coordinators, &agent).await;
    store.agent_manager_instance = Some(first.token.instance_id.clone());
    let original = first
        .run(store.claim_agent_work(&agent.id, "old-worker", NOW.into(), LONG_LEASE.into()))
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(store.due_agent_work(NOW, 10).await.unwrap().is_empty());
    let ownership = store.agent_ownership.as_ref().unwrap();
    ownership
        .heartbeat(&first.token.instance_id, true)
        .await
        .unwrap();
    expire(ownership, &first.token).await;
    let successor = acquire(&store, &coordinators, &agent).await;
    store.agent_manager_instance = Some(successor.token.instance_id.clone());
    assert_eq!(
        store.due_agent_work(NOW, 10).await.unwrap(),
        vec![agent.id.clone()]
    );
    let next = successor
        .run(store.claim_agent_work(&agent.id, "new-worker", NOW.into(), LONG_LEASE.into()))
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(
        next.work.lease.unwrap().fence,
        original.work.lease.unwrap().fence + 1
    );
    assert!(matches!(
        first
            .run(store.claim_agent_work(&agent.id, "old-worker", NOW.into(), LONG_LEASE.into()))
            .await
            .unwrap(),
        Err(SandboxStoreError::Conflict)
    ));
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn sandbox_checkpoint_publication_fences_every_captured_remote_owner() {
    use crate::store::sandbox_mongo_integration_tests::sandbox_lifecycle_tests::{
        checkpoint_for_agents, checkpoint_mutation, claimed_pause_fixture,
    };
    use layer2_core::{
        agent_ownership::{observe_transport, transport_observation_scope},
        sandbox::{
            store::{SandboxEventStore, SandboxLifecycleExecutionStore, SandboxStore},
            SandboxEventKind,
        },
    };
    let (client, name, off) = database().await;
    let (sandbox, agents) = claimed_pause_fixture(&off, "owner-checkpoint").await;
    let store = off.enable_agent_ownership("member-0".into()).await.unwrap();
    let coordinators = members(&store, 3).await;
    let mut authorities = Vec::new();
    for agent in &agents {
        authorities.push(acquire(&store, &coordinators, agent).await);
    }
    let mutation = checkpoint_mutation(&sandbox, &agents, checkpoint_for_agents(&agents, 1));
    let checkpoint_filter = doc! { "sandbox_id": sandbox.id.as_str() };
    let events = store.sandbox_events(&sandbox.id, 0, 20).await.unwrap();
    assert_eq!(
        store
            .checkpoints
            .count_documents(checkpoint_filter.clone())
            .await
            .unwrap(),
        0
    );
    assert!(matches!(
        store.publish_sandbox_checkpoint(mutation.clone()).await,
        Err(SandboxStoreError::Conflict)
    ));
    let ownership = store.agent_ownership.as_ref().unwrap();
    // Expire only one capture while keeping the other epoch valid.
    expire(ownership, &authorities[0].token).await;
    let failed = transport_observation_scope(async {
        for authority in &authorities {
            observe_transport(authority.token.clone()).unwrap();
        }
        store.publish_sandbox_checkpoint(mutation.clone()).await
    })
    .await;
    assert!(matches!(failed, Err(SandboxStoreError::Conflict)));
    assert_eq!(
        store.get_sandbox(&sandbox.id).await.unwrap(),
        Some(sandbox.clone())
    );
    assert_eq!(
        store.sandbox_events(&sandbox.id, 0, 20).await.unwrap(),
        events
    );
    assert_eq!(
        store
            .checkpoints
            .count_documents(checkpoint_filter.clone())
            .await
            .unwrap(),
        0
    );
    // A fresh capture from the new epoch permits the exact existing publication.
    let coordinator = coordinators
        .iter()
        .find(|coordinator| coordinator.instance_id == authorities[0].token.instance_id)
        .unwrap();
    coordinator
        .handoff(&authorities[0], async { Ok(()) })
        .await
        .unwrap();
    let preferred = preferred(ownership, &agents[0].id).await;
    let current = coordinators
        .iter()
        .find(|coordinator| coordinator.instance_id == preferred)
        .unwrap();
    let renewed = current.acquire(&agents[0].id).await.unwrap().unwrap();
    assert!(renewed.token.epoch > authorities[0].token.epoch);
    authorities[0] = renewed;
    let expected = mutation.sandbox.clone();
    let checkpoint = mutation.checkpoint.clone();
    transport_observation_scope(async {
        for authority in &authorities {
            observe_transport(authority.token.clone()).unwrap();
        }
        store.publish_sandbox_checkpoint(mutation).await
    })
    .await
    .unwrap();
    assert_eq!(
        store.get_sandbox(&sandbox.id).await.unwrap(),
        Some(expected)
    );
    assert_eq!(
        store
            .checkpoints
            .count_documents(checkpoint_filter)
            .await
            .unwrap(),
        1
    );
    let published = store.sandbox_events(&sandbox.id, 0, 20).await.unwrap();
    assert_eq!(published.len(), events.len() + 1);
    assert_eq!(&published[..events.len()], events.as_slice());
    assert_eq!(
        published.last().unwrap().kind,
        SandboxEventKind::CheckpointSealed {
            object_reference: checkpoint.object_reference
        }
    );
    client.database(&name).drop().await.unwrap();
}
