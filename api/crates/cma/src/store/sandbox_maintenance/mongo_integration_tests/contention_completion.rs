//! Drive admitted pauses through the real fenced store using fixture receipts.
use super::*;
use layer2_core::sandbox::store::{
    SandboxCheckpointAgentGuard, SandboxCheckpointMutation, SandboxLifecycleAgentMutation,
    SandboxLifecycleAgentScope, SandboxLifecycleExecutionStore, SandboxLifecycleMutation,
    SandboxMutationGuard, SandboxWorkerStore,
};
use layer2_core::sandbox::SandboxWorkStep;

pub(super) async fn complete(store: &MongoSandboxStore, id: &SandboxId) {
    let agent = store
        .get_agent(&AgentId(format!("agt_{id}")))
        .await
        .unwrap()
        .unwrap();
    let checkpoint = crate::store::sandbox_mongo_integration_tests::sandbox_lifecycle_tests::checkpoint_for_agents(std::slice::from_ref(&agent), 1);
    for stage in 0..4 {
        let claimed = store
            .claim_sandbox_work(
                id,
                "pause-worker",
                LATER.into(),
                crate::store::sandbox_mongo_integration_tests::work_lease_deadline(),
            )
            .await
            .unwrap()
            .unwrap();
        let fence = claimed.work.lease.as_ref().unwrap().fence;
        let mut next = claimed.clone();
        if stage == 0 {
            next.publish_lifecycle_checkpoint(
                checkpoint.clone(),
                "pause-worker",
                fence,
                LATER.into(),
            )
            .unwrap();
            store
                .publish_sandbox_checkpoint(SandboxCheckpointMutation {
                    guard: SandboxMutationGuard::capture(&claimed),
                    sandbox: next,
                    agents: vec![SandboxCheckpointAgentGuard::capture(&agent)],
                    nonblocking: false,
                    checkpoint: checkpoint.clone(),
                    appended_at: LATER.into(),
                })
                .await
                .unwrap();
            continue;
        }
        let mut agents = Vec::new();
        let mut events = Vec::new();
        match stage {
            1 => {
                next.complete_lifecycle_agent_pause("pause-worker", fence, LATER.into())
                    .unwrap();
                let mut paused = agent.clone();
                paused
                    .transition(AgentStatus::Paused, LATER.into(), None)
                    .unwrap();
                paused.conversation.last_seq += 1;
                agents.push(SandboxLifecycleAgentMutation {
                    expected_revision: agent.revision,
                    agent: paused,
                    logs: vec![AgentLogKind::StatusChanged {
                        from: AgentStatus::Ready,
                        to: AgentStatus::Paused,
                    }],
                });
                events.push(SandboxEventKind::AgentStatusChanged {
                    agent_id: agent.id.clone(),
                    from: AgentStatus::Ready,
                    to: AgentStatus::Paused,
                });
            }
            2 => next
                .complete_lifecycle_runtime_teardown("pause-worker", fence, LATER.into())
                .unwrap(),
            _ => {
                next.publish_lifecycle_paused("pause-worker", fence, LATER.into())
                    .unwrap();
                events.push(SandboxEventKind::StatusChanged {
                    from: SandboxStatus::Pausing,
                    to: SandboxStatus::Paused,
                });
            }
        }
        if stage < 3 {
            events.push(SandboxEventKind::WorkStepCompleted {
                step: SandboxWorkStep::RunLifecycleOperation,
            });
        }
        store
            .commit_sandbox_lifecycle(SandboxLifecycleMutation {
                guard: SandboxMutationGuard::capture(&claimed),
                sandbox: next,
                agents,
                agent_scope: if stage == 1 {
                    SandboxLifecycleAgentScope::AllLive
                } else {
                    SandboxLifecycleAgentScope::Touched
                },
                appended_at: LATER.into(),
                events,
            })
            .await
            .unwrap();
    }
    let paused = store.get_sandbox(id).await.unwrap().unwrap();
    assert_eq!(paused.status, SandboxStatus::Paused);
    assert!(paused.runtime.is_none() && paused.work.lease.is_none());
    assert_eq!(
        store.get_agent(&agent.id).await.unwrap().unwrap().status,
        AgentStatus::Paused
    );
}
