use super::*;
use crate::agent_manager::relay::tests::{acquire, cluster, Instance};
use crate::sandbox::live_agents::LiveAgentTarget;
use layer2_core::sandbox::store::AgentStore;
use layer2_core::sandbox::{Agent, AgentStatus, SandboxLifecycleOperation, SandboxRevivePhase};

async fn reopening() -> (Harness, Sandbox, Agent, SystemTime) {
    let harness = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    let created = harness.create().await;
    let now = start_time(&created);
    let active = harness.drive_active(&created.id, now).await;
    let agent = lifecycle_recovery_tests::prepare_agent(&harness, &active, now, 1).await;
    harness
        .store
        .schedule_sandbox_lifecycle(SandboxLifecycleCommand::pause(
            active.id.clone(),
            active.owner.clone(),
            rfc3339_at(now),
            None,
        ))
        .await
        .unwrap();
    for _ in 0..4 {
        harness.run(&active.id, now).await;
    }
    harness
        .store
        .schedule_sandbox_lifecycle(SandboxLifecycleCommand::revive(
            active.id.clone(),
            active.owner.clone(),
            rfc3339_at(now),
            SandboxReviveCause::ExplicitRequest,
        ))
        .await
        .unwrap();
    for _ in 0..12 {
        let parent = harness.stored(&active.id).await;
        if matches!(
            parent.lifecycle.operation,
            Some(SandboxLifecycleOperation::Revive {
                phase: SandboxRevivePhase::ConversationReopen,
                ..
            })
        ) {
            let agent = harness.store.get_agent(&agent.id).await.unwrap().unwrap();
            return (harness, parent, agent, now);
        }
        harness.run(&active.id, now).await;
    }
    panic!("fixture never reached conversation reopen")
}

fn workers(
    harness: &Harness,
    instances: &[Instance],
) -> Vec<(Arc<SandboxWorker>, Arc<LiveAgentRegistry>)> {
    instances
        .iter()
        .enumerate()
        .map(|(index, instance)| {
            let registry = Arc::new(LiveAgentRegistry::default());
            registry.install_ownership(instance.coordinator.clone());
            let worker = SandboxWorker::new(
                SandboxWorkerDependencies {
                    store: harness.store.clone(),
                    adapter: harness.adapter.clone(),
                    credentials: harness.credentials.clone(),
                    workspaces: Arc::new(WorkspaceRecorder {
                        actions: harness.actions.clone(),
                    }),
                    agents: Arc::new(AgentStopRecorder {
                        actions: harness.actions.clone(),
                    }),
                    registry: registry.clone(),
                    updates: DurableAgentUpdateRecorder::new(harness.store.clone()),
                    operations: AgentOperationService::new(harness.store.clone(), registry.clone()),
                },
                SandboxWorkerConfig::new(
                    format!("replica-{index}"),
                    "test",
                    runtime_image(),
                    harness.policy.clone(),
                ),
            );
            instance
                .relay
                .install_reopen(worker.clone(), harness.store.clone());
            (worker, registry)
        })
        .collect()
}

fn target(parent: &Sandbox, agent: &Agent) -> LiveAgentTarget {
    LiveAgentTarget {
        agent_id: agent.id.clone(),
        conversation_id: agent.conversation.conversation_id.clone().unwrap(),
        runtime_generation: parent
            .runtime
            .as_ref()
            .unwrap()
            .provision_generation
            .unwrap(),
    }
}

async fn until(mut condition: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn lifecycle_worker_on_any_replica_reopens_and_activates_only_at_owner() {
    use layer2_core::agent_ownership::{candidates, OwnershipStore};
    for count in [2, 3] {
        let (harness, parent, agent, now) = reopening().await;
        let (store, instances) = cluster(count).await;
        let preferred = candidates(&agent.id, &store.membership().await.unwrap(), None).unwrap()[0]
            .instance_id
            .clone();
        let owner = instances
            .iter()
            .position(|instance| instance.coordinator.instance_id == preferred)
            .unwrap();
        assert!(store.owner(&agent.id).await.unwrap().is_none());
        let workers = workers(&harness, &instances);
        let caller = (owner + 1) % count;
        let publication_at = now + Duration::from_secs(30);
        assert!(workers[caller]
            .0
            .run_one(&parent.id, publication_at)
            .await
            .unwrap());
        let ready = harness.store.get_agent(&agent.id).await.unwrap().unwrap();
        assert_eq!(ready.status, AgentStatus::Ready);
        assert_eq!(
            ready
                .work
                .receipts
                .iter()
                .find(
                    |receipt| receipt.step == layer2_core::sandbox::AgentWorkStep::OpenConversation
                )
                .unwrap()
                .completed_at,
            rfc3339_at(publication_at)
        );
        let target = target(&parent, &ready);
        until(|| workers[owner].1.published_target_is_attached(&target)).await;
        for (index, (_, registry)) in workers.iter().enumerate() {
            assert_eq!(registry.target_is_attached(&target), index == owner);
        }
        assert!(matches!(
            harness.stored(&parent.id).await.lifecycle.operation,
            Some(SandboxLifecycleOperation::Revive {
                phase: SandboxRevivePhase::QueuedMessageDelivery,
                ..
            })
        ));
    }
}

#[tokio::test]
async fn uncommitted_owner_reopen_is_idempotent_and_rolls_back_on_lease_loss() {
    use layer2_core::agent_ownership::{observed_transport, transport_observation_scope};
    for count in [2, 3] {
        let (harness, parent, agent, now) = reopening().await;
        let (store, instances) = cluster(count).await;
        let (owner, authority) = acquire(&store, &instances, &agent.id).await;
        let workers = workers(&harness, &instances);
        let caller = (owner + 1) % count;
        let parent = harness
            .store
            .claim_sandbox_work(
                &parent.id,
                "remote-lifecycle",
                rfc3339_at(now),
                rfc3339_at(SystemTime::now() + Duration::from_secs(60)),
            )
            .await
            .unwrap()
            .unwrap();
        let target = target(&parent, &agent);
        transport_observation_scope(async {
            let receipt = instances[caller]
                .relay
                .reopen_agent(&parent, &agent.id)
                .await
                .unwrap();
            assert_eq!(observed_transport(&agent.id), Some(authority.token.clone()));
            assert!(workers[owner].1.target_is_attached(&target));
            assert!(!workers[owner].1.published_target_is_attached(&target));
            assert_eq!(
                instances[caller]
                    .relay
                    .reopen_agent(&parent, &agent.id)
                    .await
                    .unwrap(),
                receipt
            );
            let mut obsolete = parent.clone();
            obsolete.revision += 1;
            assert!(instances[caller]
                .relay
                .reopen_agent(&obsolete, &agent.id)
                .await
                .is_err());
        })
        .await;
        authority.revoke();
        until(|| !workers[owner].1.target_is_attached(&target)).await;
        assert_eq!(
            harness
                .store
                .get_agent(&agent.id)
                .await
                .unwrap()
                .unwrap()
                .status,
            AgentStatus::Paused
        );
    }
}
