use crate::store::sandbox_mongo_integration_tests::work_lease_deadline;
use std::sync::Arc;

use layer2_core::sandbox::store::{
    SandboxCheckpointAgentGuard, SandboxCheckpointMutation, SandboxLifecycleAgentMutation,
    SandboxLifecycleAgentScope, SandboxLifecycleCommand, SandboxLifecycleCommandDisposition,
    SandboxLifecycleCommandResult, SandboxLifecycleCommandStore, SandboxLifecycleExecutionStore,
    SandboxLifecycleMutation, SandboxMutationGuard,
};
use layer2_core::sandbox::{
    Agent, AgentLogKind, RuntimeBinding, SandboxCheckpointConversation, SandboxCheckpointRef,
    SandboxLifecycleOperation, SandboxPausePhase, SandboxReviveCause,
};
use layer3_adapter::{CheckpointReceipt, ConversationSnapshot, WriterState};
use sha2::{Digest, Sha256};

use super::*;
#[path = "parent_readiness_wake_tests.rs"]
mod parent_readiness_wake_tests;
#[path = "sandbox_lifecycle_wake_tests.rs"]
mod wake_tests;

mod commit_tests;
pub(super) mod failed_result_tests;
mod reopen_deadline_tests;

fn checkpoint(epoch: u64) -> SandboxCheckpointRef {
    let archive = b"durable conversation archive".to_vec();
    let capture = SandboxCheckpointConversation::new(
        AgentId("agt_checkpoint".into()),
        "conversation-checkpoint",
        ConversationSnapshot {
            conversation_id: "conversation-checkpoint".into(),
            archive_sha256: format!("{:x}", Sha256::digest(&archive)),
            archive,
        },
    )
    .unwrap();
    SandboxCheckpointRef::seal(
        &CheckpointReceipt {
            epoch,
            object_reference: format!("checkpoints/{epoch}"),
            sha256: "b".repeat(64),
            writers: WriterState::Held,
        },
        &[capture],
        NOW,
    )
    .unwrap()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn lifecycle_recapture_atomically_fences_already_paused_rows_without_duplicate_logs() {
    let (client, name) = database("lifecycle_recapture").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (claimed, agents) = claimed_pause_fixture(&store, "recapture").await;
    store
        .publish_sandbox_checkpoint(checkpoint_mutation(
            &claimed,
            &agents,
            checkpoint_for_agents(&agents, 1),
        ))
        .await
        .unwrap();
    let claimed = store
        .claim_sandbox_work(&claimed.id, "worker", MINUTE.into(), work_lease_deadline())
        .await
        .unwrap()
        .unwrap();
    store
        .commit_sandbox_lifecycle(pause_agents_mutation(&claimed, &agents))
        .await
        .unwrap();
    let mut parked = Vec::new();
    for agent in agents {
        parked.push(store.get_agent(&agent.id).await.unwrap().unwrap());
    }
    let claimed = store
        .claim_sandbox_work(&claimed.id, "worker", MINUTE.into(), work_lease_deadline())
        .await
        .unwrap()
        .unwrap();
    let lease = claimed.work.lease.as_ref().unwrap();
    let mut renewed = claimed.clone();
    renewed
        .retry_lifecycle_after_writer_resume(
            MINUTE.into(),
            &lease.holder,
            lease.fence,
            MINUTE.into(),
        )
        .unwrap();
    store
        .commit_sandbox_lifecycle(SandboxLifecycleMutation {
            guard: SandboxMutationGuard::capture(&claimed),
            sandbox: renewed.clone(),
            agents: Vec::new(),
            agent_scope: SandboxLifecycleAgentScope::Touched,
            appended_at: MINUTE.into(),
            events: vec![SandboxEventKind::WorkStepFailed {
                step: SandboxWorkStep::RunLifecycleOperation,
                failure: TypedFailure {
                    class: "retryable_transient".into(),
                    detail: "writers resumed before teardown".into(),
                },
            }],
        })
        .await
        .unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    assert_eq!(store.get_sandbox(&claimed.id).await.unwrap(), Some(renewed));
    let claimed = store
        .claim_sandbox_work(&claimed.id, "worker", MINUTE.into(), work_lease_deadline())
        .await
        .unwrap()
        .unwrap();
    store
        .publish_sandbox_checkpoint(checkpoint_mutation(
            &claimed,
            &parked,
            checkpoint_for_agents(&parked, 2),
        ))
        .await
        .unwrap();
    let claimed = store
        .claim_sandbox_work(&claimed.id, "worker", MINUTE.into(), work_lease_deadline())
        .await
        .unwrap()
        .unwrap();
    let valid = pause_agents_mutation(&claimed, &parked);
    let mut partial = valid.clone();
    partial.agents.pop();
    assert!(store.commit_sandbox_lifecycle(partial).await.is_err());
    assert_eq!(
        store.get_sandbox(&claimed.id).await.unwrap(),
        Some(claimed.clone())
    );
    for agent in &parked {
        assert_eq!(
            store.get_agent(&agent.id).await.unwrap(),
            Some(agent.clone())
        );
    }
    store.commit_sandbox_lifecycle(valid.clone()).await.unwrap();
    for change in &valid.agents {
        assert_eq!(
            store.get_agent(&change.agent.id).await.unwrap(),
            Some(change.agent.clone())
        );
        assert!(change.logs.is_empty());
        assert_eq!(
            store
                .agent_log(&change.agent.id, 0, 10)
                .await
                .unwrap()
                .len(),
            1
        );
    }
    let stored = store.get_sandbox(&claimed.id).await.unwrap().unwrap();
    assert_eq!(
        stored.lifecycle.checkpoints.latest.as_ref().unwrap().epoch,
        2
    );
    // Publication retains the complete replacement and reclaims superseded
    // metadata. Recapture must not restore the historical two-generation policy.
    assert!(stored.lifecycle.checkpoints.previous.is_none());
    assert_eq!(
        store
            .checkpoints
            .count_documents(doc! {
                "sandbox_id": claimed.id.as_str(), "record.epoch": 1_i64,
            })
            .await
            .unwrap(),
        0
    );
    let events = store.sandbox_events(&claimed.id, 0, 100).await.unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event.kind, SandboxEventKind::AgentStatusChanged { .. }))
            .count(),
        2
    );
    client.database(&name).drop().await.unwrap();
}

pub(in crate::store) async fn claimed_pause_fixture(
    store: &MongoSandboxStore,
    suffix: &str,
) -> (Sandbox, Vec<Agent>) {
    let mut value = sandbox(&format!("sbx_lifecycle_{suffix}"));
    value.name = format!("lifecycle-{suffix}");
    value.runtime = Some(RuntimeBinding {
        boot_id: None,
        runtime_id: format!("runtime-{suffix}"),
        runtime_version: "image-v1".into(),
        provision_generation: Some(1),
        bound_at: NOW.into(),
    });
    value.lifecycle.runtime_generation_high_water = 1;
    value.work = SandboxWorkState::new(SandboxWorkStep::None, NOW.into());
    store.create_sandbox(value.clone()).await.unwrap();
    let agents = vec![
        lifecycle_agent(&value, &format!("agt_{suffix}_a"), "conversation-a"),
        lifecycle_agent(&value, &format!("agt_{suffix}_b"), "conversation-b"),
    ];
    for agent in &agents {
        store.create_agent(agent.clone()).await.unwrap();
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
        .claim_sandbox_work(&value.id, "worker", NOW.into(), work_lease_deadline())
        .await
        .unwrap()
        .unwrap();
    (claimed, agents)
}

fn lifecycle_agent(sandbox: &Sandbox, id: &str, conversation_id: &str) -> Agent {
    let (_, mut agent) = worktree_and_agent(&sandbox.id);
    agent.id = AgentId(id.into());
    agent.worktree_id = WorktreeId(format!("wkt_{id}"));
    agent.workspace_id = WorkspaceId(format!("wks_{id}"));
    agent.owner = sandbox.owner.clone();
    agent.creation_key = format!("create-{id}");
    agent.status = AgentStatus::Ready;
    agent.conversation.conversation_id = Some(conversation_id.into());
    agent.conversation.runtime_generation = Some(1);
    agent.conversation.last_seq = 0;
    agent.work = AgentWorkState::new(AgentWorkStep::None, NOW.into());
    agent.ready_at = Some(NOW.into());
    agent
}

pub(in crate::store) fn checkpoint_for_agents(
    agents: &[Agent],
    epoch: u64,
) -> SandboxCheckpointRef {
    let captures = agents
        .iter()
        .map(|agent| {
            let archive = format!("archive-{}-{epoch}", agent.id).into_bytes();
            let conversation_id = agent.conversation.conversation_id.clone().unwrap();
            SandboxCheckpointConversation::new(
                agent.id.clone(),
                conversation_id.clone(),
                ConversationSnapshot {
                    conversation_id,
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
            object_reference: format!("checkpoints/sandbox-{epoch}"),
            sha256: format!("{epoch:064x}"),
            writers: WriterState::Held,
        },
        &captures,
        NOW,
    )
    .unwrap()
}

pub(in crate::store) fn checkpoint_mutation(
    claimed: &Sandbox,
    agents: &[Agent],
    checkpoint: SandboxCheckpointRef,
) -> SandboxCheckpointMutation {
    let mut next = claimed.clone();
    next.lifecycle
        .checkpoints
        .publish(checkpoint.clone())
        .unwrap();
    let Some(SandboxLifecycleOperation::Pause {
        phase,
        checkpoint_epoch,
        ..
    }) = next.lifecycle.operation.as_mut()
    else {
        panic!("fixture must be pausing");
    };
    *phase = SandboxPausePhase::PauseAgents;
    *checkpoint_epoch = Some(checkpoint.epoch);
    let lease = claimed.work.lease.as_ref().unwrap();
    next.work
        .advance(
            &lease.holder,
            lease.fence,
            SandboxWorkStep::RunLifecycleOperation,
            NOW.into(),
        )
        .unwrap();
    next.revision += 1;
    next.updated_at = NOW.into();
    SandboxCheckpointMutation {
        guard: SandboxMutationGuard::capture(claimed),
        agents: agents
            .iter()
            .map(SandboxCheckpointAgentGuard::capture)
            .collect(),
        nonblocking: false,
        checkpoint,
        sandbox: next,
        appended_at: NOW.into(),
    }
}

pub(super) fn pause_agents_mutation(
    claimed: &Sandbox,
    agents: &[Agent],
) -> SandboxLifecycleMutation {
    let mut next_sandbox = claimed.clone();
    let Some(SandboxLifecycleOperation::Pause { phase, .. }) =
        next_sandbox.lifecycle.operation.as_mut()
    else {
        panic!("fixture must be pausing");
    };
    *phase = SandboxPausePhase::TeardownRuntime;
    let lease = claimed.work.lease.as_ref().unwrap();
    next_sandbox
        .work
        .advance(
            &lease.holder,
            lease.fence,
            SandboxWorkStep::RunLifecycleOperation,
            MINUTE.into(),
        )
        .unwrap();
    next_sandbox.revision += 1;
    next_sandbox.updated_at = MINUTE.into();

    let mut events = Vec::new();
    let agents = agents
        .iter()
        .map(|current| {
            let mut next = current.clone();
            if current.status == AgentStatus::Paused {
                next.revision += 1;
                next.updated_at = MINUTE.into();
                return SandboxLifecycleAgentMutation {
                    expected_revision: current.revision,
                    agent: next,
                    logs: Vec::new(),
                };
            }
            next.transition(AgentStatus::Paused, MINUTE.into(), None)
                .unwrap();
            next.conversation.last_seq += 1;
            events.push(SandboxEventKind::AgentStatusChanged {
                agent_id: current.id.clone(),
                from: current.status,
                to: AgentStatus::Paused,
            });
            SandboxLifecycleAgentMutation {
                expected_revision: current.revision,
                agent: next,
                logs: vec![AgentLogKind::StatusChanged {
                    from: current.status,
                    to: AgentStatus::Paused,
                }],
            }
        })
        .collect();
    events.push(SandboxEventKind::WorkStepCompleted {
        step: SandboxWorkStep::RunLifecycleOperation,
    });
    SandboxLifecycleMutation {
        guard: SandboxMutationGuard::capture(claimed),
        sandbox: next_sandbox,
        agents,
        agent_scope: SandboxLifecycleAgentScope::AllLive,
        appended_at: MINUTE.into(),
        events,
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn retirement_result_uses_atomic_admission_on_every_replica_after_retry_and_restart() {
    use layer2_core::sandbox::SandboxLifecycleResult;
    let (client, name) = database("retirement_result").await;
    let first = MongoSandboxStore::open(&client, &name).await.unwrap();
    let before = sandbox("sbx_result");
    first.create_sandbox(before.clone()).await.unwrap();
    assert!(first
        .retirement_admitted_at(&before.id)
        .await
        .unwrap()
        .is_none());
    let mut retiring = before.clone();
    retiring.begin_retirement(MINUTE.into()).unwrap();
    first
        .commit(SandboxTransaction::SaveSandboxAndAppend {
            command_revival: None,
            released_runtime: None,
            expected_revision: before.revision,
            sandbox: Box::new(retiring.clone()),
            event_at: MINUTE.into(),
            event: SandboxEventKind::StatusChanged {
                from: before.status,
                to: SandboxStatus::Retiring,
            },
        })
        .await
        .unwrap();
    let now = std::time::UNIX_EPOCH
        + std::time::Duration::from_secs(layer2_core::seconds_from_rfc3339(MINUTE).unwrap() + 90);
    let original = SandboxLifecycleResult::observe(&retiring, Some(MINUTE), now, true).unwrap();
    let claimed = first
        .claim_sandbox_work(
            &before.id,
            "retirement-worker",
            TWO_MINUTES.into(),
            work_lease_deadline(),
        )
        .await
        .unwrap()
        .unwrap();
    assert!(claimed.work.lease.is_some());
    for _ in 0..2 {
        let replica = MongoSandboxStore::open(&client, &name).await.unwrap();
        let timestamp = replica.retirement_admitted_at(&before.id).await.unwrap();
        assert_eq!(timestamp.as_deref(), Some(MINUTE));
        let stored = replica.get_sandbox(&before.id).await.unwrap().unwrap();
        assert_eq!(
            SandboxLifecycleResult::observe(&stored, timestamp.as_deref(), now, true).unwrap(),
            original
        );
    }
    client.database(&name).drop().await.unwrap();
}
