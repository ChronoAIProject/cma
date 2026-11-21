use super::*;

#[tokio::test]
async fn maintenance_throttling_keeps_connection_loss_recovery_immediate() {
    let harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    let ready = completed_agent(&harness).await;
    let worker = AgentWorker::new(
        AgentWorkerDependencies {
            store: harness.store.clone(),
            adapter: harness.adapter.clone(),
            skills: Arc::new(EmptyAgentSkillResolver),
            registry: harness.registry.clone(),
            updates: harness.updates.clone(),
            operations: AgentOperationService::new(harness.store.clone(), harness.registry.clone()),
        },
        AgentWorkerConfig::new("maintenance-worker", DEPLOYMENT_ID).with_managed_documents(true),
    );
    assert_eq!(worker.reconcile_connections(harness.now).await.unwrap(), 0);
    assert_eq!(worker.reconcile_connections(harness.now).await.unwrap(), 0);
    // No maintenance result can stand in for durable state or an attachment.
    assert_eq!(harness.stored(&ready.id).await, ready);
    harness.registry.fence(
        &ready.id,
        ready.conversation.runtime_generation,
        ready.work.next_fence - 1,
    );
    assert_eq!(worker.reconcile_connections(harness.now).await.unwrap(), 1);
    assert_eq!(
        harness.stored(&ready.id).await.work.step,
        AgentWorkStep::PublishConnection
    );
}
