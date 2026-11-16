use super::*;

struct HeldPreparation {
    inner: Arc<OwnedExecution>,
    entered: Notify,
    release: Notify,
}

#[async_trait::async_trait]
impl AgentCommandExecution for HeldPreparation {
    async fn attempt_authority(
        &self,
        command: &AdmittedAgentCommand,
    ) -> Result<Option<Arc<Authority>>, CommandPreparationError> {
        self.inner.attempt_authority(command).await
    }

    async fn check_queued(&self, command: &AdmittedAgentCommand) -> CommandReadiness {
        self.inner.check_queued(command).await
    }

    async fn prepare_queued(
        &self,
        command: &AdmittedAgentCommand,
    ) -> Result<(), CommandPreparationError> {
        self.entered.notify_one();
        self.release.notified().await;
        self.inner.prepare_queued(command).await
    }

    async fn execute(&self, _: &AdmittedAgentCommand) -> CommandAttempt {
        panic!("a paused Agent must not dispatch input while preparation is pending")
    }
}

#[tokio::test]
async fn ownership_loss_during_preparation_discards_late_refusal_before_successor_rechecks() {
    let (store, agent, parent) = fixture(AgentStatus::Paused).await;
    let admitted = admit(
        &store,
        &agent,
        &parent,
        "prepare-owner-loss",
        AgentCommand::message("one"),
    )
    .await;
    let (sender, inputs) = recording_sender(Ok(()));
    let operations = AgentOperationService::new(
        store.clone(),
        TestRuntimeRegistry::new(full_capabilities(), sender),
    );
    let inner = Execution::new(store.clone(), operations, Behavior::Execute);
    let owners = MemoryOwnershipStore::default();
    let a = start(owners.client(), "preparing-owner").await;
    let authority = a.acquire(&agent.id).await.unwrap().unwrap();
    let b = start(owners.client(), "preparing-successor").await;
    let execution = Arc::new(HeldPreparation {
        inner: OwnedExecution::new(inner.clone(), a.clone(), true, false),
        entered: Notify::new(),
        release: Notify::new(),
    });
    let worker =
        AgentCommandWorker::new(store.clone(), execution.clone(), "preparing-worker".into())
            .unwrap();
    let id = admitted.receipt.envelope.command_id.clone();
    let task = tokio::spawn(async move { worker.run_scheduled(&id).await });
    tokio::time::timeout(Duration::from_secs(2), execution.entered.notified())
        .await
        .unwrap();

    a.begin_drain().await;
    owners.release(&authority.token).await.unwrap();
    let successor = b.acquire(&agent.id).await.unwrap().unwrap();
    assert!(successor.token.epoch > authority.token.epoch);
    execution.release.notify_one();
    let delay = tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let id = &admitted.receipt.envelope.command_id;
    assert_eq!(store.get_command(id).await.unwrap(), Some(admitted.clone()));
    assert!(!delay.is_zero() && delay <= Duration::from_secs(2));
    assert_eq!(
        store.get_agent(&agent.id).await.unwrap(),
        Some(agent.clone())
    );
    assert!(inputs.lock().unwrap().is_empty());

    let replacement = OwnedExecution::new(inner, b, true, false);
    replacement.worker().run_scheduled(id).await.unwrap();
    let refused = store.get_command(id).await.unwrap().unwrap();
    assert_eq!(refused.receipt, admitted.receipt);
    assert_eq!(refused.key_sha256, admitted.key_sha256);
    assert_eq!(refused.queue_deadline, admitted.queue_deadline);
    assert_eq!(refused.status, AgentResponseStatus::Failed);
    assert!(refused.work.lease.is_none() && refused.work.dispatch.is_none());
    assert_eq!(
        refused.work.failure.unwrap().code,
        CommandFailureCode::ExecutionFailed
    );
    assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(agent));
    assert!(inputs.lock().unwrap().is_empty());
    assert_eq!(
        replacement.scopes.lock().unwrap().as_slice(),
        [Some(successor.token.clone()), Some(successor.token.clone())]
    );
}
