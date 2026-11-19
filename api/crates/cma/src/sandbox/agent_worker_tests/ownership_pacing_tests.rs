use super::super::lifecycle_failure::{CauseClass, DetailCode, LifecycleAttempt, Origin, Phase};
use super::*;
use layer2_core::agent_ownership::{
    test_support::MemoryOwnershipStore, Member, OwnershipCoordinator, PROTOCOL_VERSION,
};
use layer3_adapter::owner_protocol::{ReceiverRevocation, TransportAuthority};

struct Receiver;
#[async_trait::async_trait]
impl ReceiverRevocation for Receiver {
    fn identity(&self) -> String {
        "test-binding".into()
    }
    async fn revoke(&self) -> layer3_adapter::Result<()> {
        Err(AdapterError::Unavailable("test refusal".into()))
    }
}

async fn runner(
    h: &Harness,
    id: &AgentId,
    enabled: bool,
    cancelled: bool,
) -> (Arc<AgentWorker>, Arc<OwnershipCoordinator>) {
    let coordinator = OwnershipCoordinator::register_with_cleanup_pacing(
        Arc::new(MemoryOwnershipStore::default()),
        Member {
            instance_id: "pacing-fixture".into(),
            protocol: PROTOCOL_VERSION,
            relay_address: "http://owner.invalid".into(),
            expires_at_ms: 0,
            draining: false,
            owned_agents: 0,
            max_owned_agents: 8,
        },
        enabled,
    )
    .await
    .unwrap();
    let authority = coordinator.acquire(id).await.unwrap().unwrap();
    authority.register_receiver(Arc::new(Receiver)).unwrap();
    // Cancelled custody may be reacquired only at a newer epoch, independently
    // of whether background scheduling currently chooses to make the attempt.
    if cancelled {
        TransportAuthority::revoke(authority.as_ref());
        assert!(coordinator.handoff_background(&authority).await.is_err());
    }
    let worker = AgentWorker::new(
        AgentWorkerDependencies {
            store: h.store.clone(),
            adapter: h.adapter.clone(),
            skills: Arc::new(EmptyAgentSkillResolver),
            registry: h.registry.clone(),
            updates: h.updates.clone(),
            operations: AgentOperationService::new(h.store.clone(), h.registry.clone()),
        },
        AgentWorkerConfig::new("paced-worker", DEPLOYMENT_ID)
            .with_ownership(Some(coordinator.clone())),
    );
    (worker, coordinator)
}

#[tokio::test]
async fn paced_discovery_and_stale_queue_preserve_unclaimed_work_and_healthy_peers() {
    for enabled in [false, true] {
        let h = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
        let agent = h.admit("paced-agent", vec![]).await;
        let healthy = h.admit("healthy-agent", vec![]).await;
        let (worker, coordinator) = runner(&h, &agent.id, enabled, true).await;
        let before = h.backend.requests().await.len();
        let due = worker.due_work(h.now, 100).await.unwrap();
        assert_eq!(due.contains(&agent.id), !enabled);
        assert!(due.contains(&healthy.id));
        assert!(!worker.run_scheduled(&agent.id, h.now).await.unwrap());
        assert_eq!(h.stored(&agent.id).await, agent);
        assert_eq!(h.backend.requests().await.len(), before);
        assert!(coordinator.store.owner(&agent.id).await.unwrap().is_some());
        assert!(worker.run_scheduled(&healthy.id, h.now).await.unwrap());
        assert_eq!(h.stored(&healthy.id).await.status, AgentStatus::Ready);
    }
}

#[tokio::test]
async fn real_stop_and_delete_intents_bypass_the_background_cooldown_without_losing_fences() {
    for delete in [false, true] {
        let h = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
        let agent = h.admit("control-during-cooldown", vec![]).await;
        let mut ready = h.drive_ready(&agent.id).await;
        if delete {
            h.store
                .schedule_agent_stop(layer2_core::sandbox::store::AgentStopRequest {
                    target: AgentControlTarget::from(&ready),
                    reason: AgentStopReason::Owner,
                    requested_at: rfc3339_at(h.now),
                })
                .await
                .unwrap();
            for _ in 0..3 {
                h.run(&agent.id, h.now).await;
            }
            ready = h.stored(&agent.id).await;
            assert_eq!(ready.status, AgentStatus::Stopped);
        }
        let (worker, coordinator) = runner(&h, &agent.id, true, true).await;
        if delete {
            h.store
                .schedule_agent_delete(AgentDeleteRequest {
                    target: AgentControlTarget::from(&ready),
                    requested_at: rfc3339_at(h.now),
                })
                .await
                .unwrap();
        } else {
            h.store
                .schedule_agent_stop(layer2_core::sandbox::store::AgentStopRequest {
                    target: AgentControlTarget::from(&ready),
                    reason: AgentStopReason::Owner,
                    requested_at: rfc3339_at(h.now),
                })
                .await
                .unwrap();
        }
        let intent = h.stored(&agent.id).await;
        let before = h.backend.requests().await.len();
        assert!(coordinator.background_cleanup_deferred(&agent.id).await);
        assert!(worker
            .due_work(h.now, 100)
            .await
            .unwrap()
            .contains(&agent.id));
        // The old exact token is still live. Normal acquisition refuses it;
        // pacing cannot turn that refusal into claimed or completed work.
        assert!(!worker.run_scheduled(&agent.id, h.now).await.unwrap());
        assert_eq!(h.stored(&agent.id).await, intent);
        assert_eq!(h.backend.requests().await.len(), before);
        assert!(coordinator.store.owner(&agent.id).await.unwrap().is_some());
        coordinator.fail().await;
        assert_eq!(
            worker.run_scheduled(&agent.id, h.now).await,
            Err(layer2_core::sandbox::store::SandboxStoreError::Conflict),
            "control must reach the normal ownership refusal, not a pacing success"
        );
        assert_eq!(h.stored(&agent.id).await, intent);
    }
}

#[tokio::test]
async fn failed_agent_handoff_records_cleanup_debt_without_replaying_terminal_work() {
    for enabled in [false, true] {
        let h = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
        let agent = h.admit("failed-agent-handoff", vec![]).await;
        h.run(&agent.id, h.now).await;
        h.run(&agent.id, h.now).await;
        h.backend
            .fail_next(WireOperation::OpenAgent, "request_rejected", "open refused");
        h.run(&agent.id, h.now).await;
        let failed = h.stored(&agent.id).await;
        assert_eq!(failed.status, AgentStatus::Failed);
        assert_eq!(failed.work.step, AgentWorkStep::None);
        let (worker, coordinator) = runner(&h, &agent.id, enabled, false).await;
        let before = h.backend.requests().await.len();
        let attempt = LifecycleAttempt::new(Phase::Execution, Some(&agent.id));
        let initial = attempt.terminated(false);
        let failure = worker
            .run_scheduled_observed(&agent.id, h.now, &attempt)
            .await
            .unwrap_err();
        assert_eq!(failure.observation.origin, Origin::Handoff);
        assert_eq!(
            failure.observation.cause_class,
            CauseClass::OwnershipUnavailable
        );
        assert_eq!(
            failure.observation.detail_code,
            DetailCode::ReceiverRevocationUnconfirmed
        );
        assert_eq!(failure.observation.attempt_id, initial.attempt_id);
        assert_eq!(failure.observation.agent_hash, initial.agent_hash);
        assert_eq!(failure.observation.continuation_index, 0);
        assert_eq!(
            failure.into_compatibility(),
            layer2_core::sandbox::store::SandboxStoreError::Conflict
        );
        assert_eq!(
            coordinator.background_cleanup_deferred(&agent.id).await,
            enabled
        );
        assert_eq!(h.stored(&agent.id).await, failed);
        assert_eq!(h.backend.requests().await.len(), before);
        assert!(coordinator.store.owner(&agent.id).await.unwrap().is_some());
    }
}
