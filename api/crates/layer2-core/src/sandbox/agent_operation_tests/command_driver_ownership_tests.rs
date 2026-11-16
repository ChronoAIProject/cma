use super::*;
use crate::agent_ownership::{
    execution_owner, test_support::MemoryOwnershipStore, Authority, Member, OwnershipCoordinator,
    OwnershipStore, OwnershipToken, PROTOCOL_VERSION,
};
use std::sync::Mutex;

#[path = "command_preparation_ownership_tests.rs"]
mod preparation;

async fn start(store: Arc<MemoryOwnershipStore>, id: &str) -> Arc<OwnershipCoordinator> {
    OwnershipCoordinator::register(
        store,
        Member {
            instance_id: id.into(),
            protocol: PROTOCOL_VERSION,
            relay_address: format!("http://{id}:8080"),
            expires_at_ms: 0,
            draining: false,
            owned_agents: 0,
            max_owned_agents: 256,
        },
    )
    .await
    .unwrap()
}

struct OwnedExecution {
    inner: Arc<Execution>,
    coordinator: Arc<OwnershipCoordinator>,
    preparation_refusal: bool,
    hold_result: bool,
    entered: Notify,
    release: Notify,
    scopes: Mutex<Vec<Option<OwnershipToken>>>,
}

impl OwnedExecution {
    fn new(
        inner: Arc<Execution>,
        coordinator: Arc<OwnershipCoordinator>,
        preparation_refusal: bool,
        hold_result: bool,
    ) -> Arc<Self> {
        Arc::new(Self {
            inner,
            coordinator,
            preparation_refusal,
            hold_result,
            entered: Notify::new(),
            release: Notify::new(),
            scopes: Mutex::default(),
        })
    }
    fn worker(self: &Arc<Self>) -> Arc<AgentCommandWorker> {
        AgentCommandWorker::new(
            self.inner.store.clone(),
            self.clone(),
            self.coordinator.instance_id.clone(),
        )
        .unwrap()
    }
    fn observe(&self) {
        self.scopes.lock().unwrap().push(execution_owner());
    }
}

#[async_trait::async_trait]
impl AgentCommandExecution for OwnedExecution {
    async fn attempt_authority(
        &self,
        command: &AdmittedAgentCommand,
    ) -> Result<Option<Arc<Authority>>, CommandPreparationError> {
        self.coordinator
            .acquire(&command.receipt.envelope.agent_id)
            .await
            .map_err(|_| CommandPreparationError::AuthorityUnavailable)?
            .map(Some)
            .ok_or(CommandPreparationError::AuthorityUnavailable)
    }
    async fn check_queued(&self, command: &AdmittedAgentCommand) -> CommandReadiness {
        self.observe();
        self.inner.check_queued(command).await
    }
    async fn prepare_queued(
        &self,
        _: &AdmittedAgentCommand,
    ) -> Result<(), CommandPreparationError> {
        self.observe();
        if self.preparation_refusal {
            Err(CommandPreparationError::Rejected(
                CommandFailureCode::ConnectionsRequired,
            ))
        } else {
            Ok(())
        }
    }
    async fn execute(&self, command: &AdmittedAgentCommand) -> CommandAttempt {
        self.observe();
        let result = self.inner.execute(command).await;
        self.entered.notify_one();
        if self.hold_result {
            self.release.notified().await;
        }
        result
    }
}

async fn complete(store: &Arc<InMemorySandboxStore>, agent: &Agent) {
    DurableAgentUpdateRecorder::new(store.clone())
        .record_update(
            &agent.id,
            3,
            &mut AgentMessageAssembly::default(),
            ConversationUpdate::TurnEnded(PromptResponse::new(TurnStopReason::EndTurn)),
            crate::now_rfc3339(),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn dispatched_reconciliation_requires_owner_and_never_replays_input() {
    let (store, agent, parent) = fixture(AgentStatus::Ready).await;
    let admitted = admit(
        &store,
        &agent,
        &parent,
        "owned-reconcile",
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
    let a = start(owners.client(), "owner").await;
    let authority = a.acquire(&agent.id).await.unwrap().unwrap();
    let b = start(owners.client(), "peer").await;
    let owner = OwnedExecution::new(inner.clone(), a, false, false);
    let peer = OwnedExecution::new(inner.clone(), b, false, false);
    let id = &admitted.receipt.envelope.command_id;
    owner.worker().run_scheduled(id).await.unwrap();
    assert_eq!(inputs.lock().unwrap().len(), 1);
    complete(&store, &agent).await;
    let before = store.get_command(id).await.unwrap().unwrap();
    assert!(before.work.dispatch.is_some());
    let delay = peer.worker().run_scheduled(id).await.unwrap();
    assert_eq!(store.get_command(id).await.unwrap(), Some(before));
    assert!(!delay.is_zero() && delay <= Duration::from_secs(2));
    owner.worker().run_scheduled(id).await.unwrap();
    let done = store.get_command(id).await.unwrap().unwrap();
    assert_eq!(done.status, AgentResponseStatus::Completed);
    assert_eq!(done.receipt, admitted.receipt);
    owner.worker().run_scheduled(id).await.unwrap();
    assert_eq!(store.get_command(id).await.unwrap(), Some(done));
    assert_eq!(inputs.lock().unwrap().len(), 1);
    assert_eq!(inner.attempts.load(Ordering::SeqCst), 1);
    assert!(peer.scopes.lock().unwrap().is_empty());
    assert!(owner
        .scopes
        .lock()
        .unwrap()
        .iter()
        .all(|token| token.as_ref() == Some(&authority.token)));
}

#[tokio::test]
async fn timeout_and_preparation_refusal_wait_for_owner_without_changing_receipt() {
    use super::super::ledger_tests::{record, reservation};
    for expired in [false, true] {
        let (store, agent, parent) = fixture(AgentStatus::Paused).await;
        let admitted = if expired {
            record(
                store
                    .reserve_command(reservation(&agent, &parent, "expired-owned"))
                    .await
                    .unwrap(),
            )
        } else {
            admit(
                &store,
                &agent,
                &parent,
                "prepare-owned",
                AgentCommand::message("one"),
            )
            .await
        };
        let (sender, inputs) = recording_sender(Ok(()));
        let operations = AgentOperationService::new(
            store.clone(),
            TestRuntimeRegistry::new(full_capabilities(), sender),
        );
        let inner = Execution::new(store.clone(), operations, Behavior::Execute);
        let owners = MemoryOwnershipStore::default();
        let a = start(owners.client(), "owner").await;
        let authority = a.acquire(&agent.id).await.unwrap().unwrap();
        let b = start(owners.client(), "peer").await;
        let peer = OwnedExecution::new(inner.clone(), b, true, false);
        let id = &admitted.receipt.envelope.command_id;
        let delay = peer.worker().run_scheduled(id).await.unwrap();
        assert_eq!(store.get_command(id).await.unwrap(), Some(admitted.clone()));
        assert!(!delay.is_zero() && delay <= Duration::from_secs(2));
        let owner = OwnedExecution::new(inner, a, true, false);
        owner.worker().run_scheduled(id).await.unwrap();
        let refused = store.get_command(id).await.unwrap().unwrap();
        assert_eq!(refused.receipt, admitted.receipt);
        assert_eq!(
            refused.work.failure.unwrap().code,
            if expired {
                CommandFailureCode::QueueTimeout
            } else {
                CommandFailureCode::ExecutionFailed
            }
        );
        assert!(inputs.lock().unwrap().is_empty());
        assert!(peer.scopes.lock().unwrap().is_empty());
        assert!(owner
            .scopes
            .lock()
            .unwrap()
            .iter()
            .all(|token| token.as_ref() == Some(&authority.token)));
    }
}

#[tokio::test]
async fn revoked_execution_cannot_settle_and_successor_reconciles_original_dispatch() {
    let (store, agent, parent) = fixture(AgentStatus::Ready).await;
    let admitted = admit(
        &store,
        &agent,
        &parent,
        "revoke-settle",
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
    let a = start(owners.client(), "owner").await;
    let authority = a.acquire(&agent.id).await.unwrap().unwrap();
    let b = start(owners.client(), "peer").await;
    let owner = OwnedExecution::new(inner.clone(), a.clone(), false, true);
    let worker = owner.worker();
    let id = admitted.receipt.envelope.command_id.clone();
    let task = tokio::spawn(async move { worker.run_scheduled(&id).await });
    tokio::time::timeout(Duration::from_secs(2), owner.entered.notified())
        .await
        .unwrap();
    complete(&store, &agent).await;
    let id = &admitted.receipt.envelope.command_id;
    let bound = store.get_command(id).await.unwrap().unwrap();
    a.begin_drain().await;
    owners.release(&authority.token).await.unwrap();
    let successor = b.acquire(&agent.id).await.unwrap().unwrap();
    assert!(successor.token.epoch > authority.token.epoch);
    owner.release.notify_one();
    let delay = tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(store.get_command(id).await.unwrap(), Some(bound.clone()));
    assert!(!delay.is_zero());
    OwnedExecution::new(inner, b, false, false)
        .worker()
        .run_scheduled(id)
        .await
        .unwrap();
    let done = store.get_command(id).await.unwrap().unwrap();
    assert_eq!(done.status, AgentResponseStatus::Completed);
    assert_eq!(done.receipt, admitted.receipt);
    assert_eq!(done.work.dispatch, bound.work.dispatch);
    assert_eq!(inputs.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn draining_or_partitioned_member_defers_expired_work_before_any_rejection() {
    use super::super::ledger_tests::{record, reservation};
    for draining in [false, true] {
        let (store, agent, parent) = fixture(AgentStatus::Paused).await;
        let admitted = record(
            store
                .reserve_command(reservation(&agent, &parent, "denied-owner"))
                .await
                .unwrap(),
        );
        let (sender, inputs) = recording_sender(Ok(()));
        let operations = AgentOperationService::new(
            store.clone(),
            TestRuntimeRegistry::new(full_capabilities(), sender),
        );
        let inner = Execution::new(store.clone(), operations, Behavior::Execute);
        let owners = MemoryOwnershipStore::default();
        let client = owners.client();
        let coordinator = start(client.clone(), "owner").await;
        if draining {
            coordinator.stop_admission();
        } else {
            client.partition(true);
        }
        let execution = OwnedExecution::new(inner, coordinator, true, false);
        let delay = execution
            .worker()
            .run_scheduled(&admitted.receipt.envelope.command_id)
            .await
            .unwrap();
        assert_eq!(
            store
                .get_command(&admitted.receipt.envelope.command_id)
                .await
                .unwrap(),
            Some(admitted)
        );
        assert!(!delay.is_zero() && delay <= Duration::from_secs(2));
        assert!(execution.scopes.lock().unwrap().is_empty());
        assert!(inputs.lock().unwrap().is_empty());
    }
}
