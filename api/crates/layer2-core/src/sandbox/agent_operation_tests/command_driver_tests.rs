use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use layer3_adapter::{AdapterError, ConversationUpdate, PromptResponse, TurnStopReason};
use tokio::sync::Notify;

use super::command_work_tests::{admit, fixture, request};
use super::fixtures::*;
use crate::sandbox::agent_io::*;
use crate::sandbox::store::{AgentLogStore, AgentStore, PageRequest};
use crate::sandbox::*;

#[path = "command_driver_failures_tests.rs"]
mod failures;
#[path = "command_driver_retry_tests.rs"]
mod retries;

#[path = "command_diagnostics_tests.rs"]
mod diagnostics;

#[path = "command_preparation_tests.rs"]
mod preparation;

enum Behavior {
    Execute,
    RejectAfterBinding,
    HangAfterBinding,
    HangBeforeBinding,
    LoseLease,
    Refuse,
    ClaimDuringCheck,
}

struct Execution {
    store: Arc<InMemorySandboxStore>,
    operations: Arc<AgentOperationService>,
    behavior: Behavior,
    checks: AtomicUsize,
    preparations: AtomicUsize,
    attempts: AtomicUsize,
    entered: Notify,
}

#[async_trait::async_trait]
impl AgentCommandExecution for Execution {
    async fn check_queued(&self, command: &AdmittedAgentCommand) -> CommandReadiness {
        self.checks.fetch_add(1, Ordering::SeqCst);
        if matches!(self.behavior, Behavior::Refuse) {
            return CommandReadiness::Reject(CommandFailureCode::AuthorityRevoked);
        }
        if matches!(self.behavior, Behavior::ClaimDuringCheck) {
            let agent = self
                .store
                .get_agent(&command.receipt.envelope.agent_id)
                .await
                .unwrap()
                .unwrap();
            self.store
                .claim_next_command(request(&agent, "competitor", &crate::now_rfc3339()))
                .await
                .unwrap();
        }
        CommandReadiness::Ready
    }

    async fn prepare_queued(
        &self,
        _: &AdmittedAgentCommand,
    ) -> Result<(), CommandPreparationError> {
        self.preparations.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn execute(&self, command: &AdmittedAgentCommand) -> CommandAttempt {
        self.attempts.fetch_add(1, Ordering::SeqCst);
        if matches!(self.behavior, Behavior::LoseLease) {
            self.store
                .release_command_claim(
                    &command.work.lease.as_ref().unwrap().claim,
                    crate::now_rfc3339(),
                )
                .await
                .unwrap();
        }
        if matches!(
            self.behavior,
            Behavior::HangBeforeBinding | Behavior::LoseLease
        ) {
            self.entered.notify_one();
            std::future::pending::<()>().await;
        }
        let result = self
            .operations
            .execute_claimed_command(&owner(), command)
            .await;
        self.entered.notify_one();
        match self.behavior {
            Behavior::RejectAfterBinding => {
                CommandAttempt::Reject(CommandFailureCode::AuthorityRevoked)
            }
            Behavior::HangAfterBinding => std::future::pending().await,
            _ if result.is_ok() => CommandAttempt::Finished,
            _ => CommandAttempt::Retry,
        }
    }
}

impl Execution {
    fn new(
        store: Arc<InMemorySandboxStore>,
        operations: Arc<AgentOperationService>,
        behavior: Behavior,
    ) -> Arc<Self> {
        Arc::new(Self {
            store,
            operations,
            behavior,
            checks: AtomicUsize::new(0),
            preparations: AtomicUsize::new(0),
            attempts: AtomicUsize::new(0),
            entered: Notify::new(),
        })
    }

    fn worker(self: &Arc<Self>, holder: &str) -> Arc<AgentCommandWorker> {
        AgentCommandWorker::new(self.store.clone(), self.clone(), holder.into()).unwrap()
    }
}

#[tokio::test]
async fn peer_without_agent_ownership_leaves_queued_command_for_the_owner() {
    use crate::agent_ownership::{
        test_support::MemoryOwnershipStore, Member, OwnershipCoordinator, DEFAULT_MAX_OWNED_AGENTS,
        PROTOCOL_VERSION,
    };

    let (store, agent, parent) = fixture(AgentStatus::Ready).await;
    let admitted = admit(
        &store,
        &agent,
        &parent,
        "queued",
        AgentCommand::message("one"),
    )
    .await;
    let original_log = store.agent_log(&agent.id, 0, 20).await.unwrap();
    let owners = Arc::new(MemoryOwnershipStore::default());
    let member = |id: &str| Member {
        instance_id: id.into(),
        protocol: PROTOCOL_VERSION,
        relay_address: format!("http://{id}.invalid"),
        expires_at_ms: 0,
        draining: false,
        owned_agents: 0,
        max_owned_agents: DEFAULT_MAX_OWNED_AGENTS,
    };
    let owner = OwnershipCoordinator::register(owners.clone(), member("owner"))
        .await
        .unwrap();
    let _authority = owner.acquire(&agent.id).await.unwrap().unwrap();
    let peer = OwnershipCoordinator::register(owners.client(), member("peer"))
        .await
        .unwrap();
    let (sender, inputs) = recording_sender(Ok(()));
    let operations = AgentOperationService::new(
        store.clone(),
        TestRuntimeRegistry::new(full_capabilities(), sender),
    );
    let execution = Execution::new(store.clone(), operations, Behavior::Execute);
    let id = &admitted.receipt.envelope.command_id;

    let delay = execution
        .worker("peer")
        .with_ownership(Some(peer))
        .run_scheduled(id)
        .await
        .unwrap();
    assert_eq!(delay, Duration::from_secs(15));
    assert_eq!(execution.checks.load(Ordering::SeqCst), 1);
    assert_eq!(execution.preparations.load(Ordering::SeqCst), 0);
    assert_eq!(execution.attempts.load(Ordering::SeqCst), 0);
    assert!(inputs.lock().unwrap().is_empty());
    assert_eq!(store.get_command(id).await.unwrap(), Some(admitted.clone()));
    assert_eq!(
        store.agent_log(&agent.id, 0, 20).await.unwrap(),
        original_log
    );

    execution
        .worker("owner")
        .with_ownership(Some(owner))
        .run_scheduled(id)
        .await
        .unwrap();
    let dispatched = store.get_command(id).await.unwrap().unwrap();
    assert_eq!(dispatched.receipt, admitted.receipt);
    assert!(dispatched.work.dispatch.is_some());
    assert_eq!(inputs.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn concurrent_workers_and_restarts_send_exactly_once_and_recover_completion() {
    for ambiguous in [false, true] {
        let (store, agent, parent) = fixture(AgentStatus::Ready).await;
        let admitted = admit(
            &store,
            &agent,
            &parent,
            "once",
            AgentCommand::message("one"),
        )
        .await;
        let reply = if ambiguous {
            Err(AdapterError::Rejected("lost acknowledgment".into()))
        } else {
            Ok(())
        };
        let (sender, inputs) = recording_sender(reply);
        let operations = AgentOperationService::new(
            store.clone(),
            TestRuntimeRegistry::new(full_capabilities(), sender),
        );
        let execution = Execution::new(store.clone(), operations, Behavior::Execute);
        let id = &admitted.receipt.envelope.command_id;
        let a = execution.worker("one");
        let b = execution.worker("two");
        let (a, b) = tokio::join!(a.run_one(id), b.run_one(id));
        assert!(a.is_ok());
        assert!(b.is_ok());
        assert_eq!(inputs.lock().unwrap().len(), 1);
        let checks = execution.checks.load(Ordering::SeqCst);
        execution.worker("restart").run_one(id).await.unwrap();
        assert_eq!(execution.checks.load(Ordering::SeqCst), checks);
        assert_eq!(inputs.lock().unwrap().len(), 1);
        let running = store.get_command(id).await.unwrap().unwrap();
        assert_eq!(running.receipt, admitted.receipt);
        assert_eq!(running.status, AgentResponseStatus::InProgress);
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
        execution.worker("completion").run_one(id).await.unwrap();
        let completed = store.get_command(id).await.unwrap().unwrap();
        assert_eq!(completed.status, AgentResponseStatus::Completed);
        assert_eq!(completed.receipt, admitted.receipt);
        assert_eq!(inputs.lock().unwrap().len(), 1);
    }
}

#[tokio::test(start_paused = true)]
async fn binding_wins_over_refusal_and_timeout_after_runtime_acceptance() {
    for behavior in [Behavior::RejectAfterBinding, Behavior::HangAfterBinding] {
        let (store, agent, parent) = fixture(AgentStatus::Ready).await;
        let admitted = admit(
            &store,
            &agent,
            &parent,
            "binding",
            AgentCommand::message("one"),
        )
        .await;
        let (sender, inputs) = recording_sender(Ok(()));
        let operations = AgentOperationService::new(
            store.clone(),
            TestRuntimeRegistry::new(full_capabilities(), sender),
        );
        let execution = Execution::new(store.clone(), operations, behavior);
        let worker = execution.worker("original");
        let id = admitted.receipt.envelope.command_id.clone();
        let task = tokio::spawn(async move { worker.run_one(&id).await });
        execution.entered.notified().await;
        for _ in 0..5 {
            tokio::time::advance(Duration::from_secs(10)).await;
            tokio::task::yield_now().await;
        }
        task.await.unwrap().unwrap();
        let current = store
            .get_command(&admitted.receipt.envelope.command_id)
            .await
            .unwrap()
            .unwrap();
        assert!(current.work.dispatch.is_some());
        assert!(current.work.failure.is_none());
        assert_eq!(current.status, AgentResponseStatus::InProgress);
        execution
            .worker("restart")
            .run_one(&admitted.receipt.envelope.command_id)
            .await
            .unwrap();
        assert_eq!(inputs.lock().unwrap().len(), 1);
        assert_eq!(execution.attempts.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test(start_paused = true)]
async fn timed_out_unbound_work_renews_then_releases_and_can_be_reclaimed() {
    let (store, agent, parent) = fixture(AgentStatus::Ready).await;
    let admitted = admit(
        &store,
        &agent,
        &parent,
        "timeout",
        AgentCommand::message("one"),
    )
    .await;
    let (sender, inputs) = recording_sender(Ok(()));
    let operations = AgentOperationService::new(
        store.clone(),
        TestRuntimeRegistry::new(full_capabilities(), sender),
    );
    let execution = Execution::new(
        store.clone(),
        operations.clone(),
        Behavior::HangBeforeBinding,
    );
    let worker = execution.worker("slow");
    let id = admitted.receipt.envelope.command_id.clone();
    let task = tokio::spawn(async move { worker.run_one(&id).await });
    execution.entered.notified().await;
    for _ in 0..3 {
        tokio::time::advance(Duration::from_secs(10)).await;
        tokio::task::yield_now().await;
    }
    let renewed = store
        .get_command(&admitted.receipt.envelope.command_id)
        .await
        .unwrap()
        .unwrap();
    assert!(renewed.revision >= admitted.revision + 3);
    assert!(renewed.work.lease.is_some());
    tokio::time::advance(Duration::from_secs(20)).await;
    task.await.unwrap().unwrap();
    let released = store
        .get_command(&admitted.receipt.envelope.command_id)
        .await
        .unwrap()
        .unwrap();
    assert!(released.work.lease.is_none());
    assert!(released.work.dispatch.is_none());
    assert!(inputs.lock().unwrap().is_empty());
    let recovered = Execution::new(store, operations, Behavior::Execute);
    recovered
        .worker("replacement")
        .run_one(&admitted.receipt.envelope.command_id)
        .await
        .unwrap();
    assert_eq!(inputs.lock().unwrap().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn lost_lease_cancels_the_attempt_without_dispatch_or_stale_refusal() {
    let (store, agent, parent) = fixture(AgentStatus::Ready).await;
    let admitted = admit(
        &store,
        &agent,
        &parent,
        "loss",
        AgentCommand::message("one"),
    )
    .await;
    let (sender, inputs) = recording_sender(Ok(()));
    let operations = AgentOperationService::new(
        store.clone(),
        TestRuntimeRegistry::new(full_capabilities(), sender),
    );
    let execution = Execution::new(store.clone(), operations, Behavior::LoseLease);
    assert_eq!(
        execution
            .worker("loser")
            .run_one(&admitted.receipt.envelope.command_id)
            .await,
        Err(CommandLedgerError::StaleClaim)
    );
    assert!(inputs.lock().unwrap().is_empty());
    let current = store
        .get_command(&admitted.receipt.envelope.command_id)
        .await
        .unwrap()
        .unwrap();
    assert!(current.work.dispatch.is_none());
    assert!(current.work.failure.is_none());
}

#[tokio::test]
async fn revocation_refuses_paused_or_busy_work_without_revive_or_dispatch() {
    for status in [AgentStatus::Paused, AgentStatus::Working] {
        let (store, agent, parent) = fixture(status).await;
        let admitted = admit(
            &store,
            &agent,
            &parent,
            "revoked",
            AgentCommand::message("one"),
        )
        .await;
        let (sender, inputs) = recording_sender(Ok(()));
        let operations = AgentOperationService::new(
            store.clone(),
            TestRuntimeRegistry::new(full_capabilities(), sender),
        );
        let execution = Execution::new(store.clone(), operations, Behavior::Refuse);
        execution
            .worker("worker")
            .run_one(&admitted.receipt.envelope.command_id)
            .await
            .unwrap();
        let current = store
            .get_command(&admitted.receipt.envelope.command_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(current.receipt, admitted.receipt);
        assert_eq!(
            current.work.failure.unwrap().code,
            CommandFailureCode::AuthorityRevoked
        );
        assert_eq!(execution.preparations.load(Ordering::SeqCst), 0);
        assert_eq!(execution.attempts.load(Ordering::SeqCst), 0);
        assert!(inputs.lock().unwrap().is_empty());
        assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(agent));
    }
}

#[tokio::test]
async fn another_replica_claiming_during_check_prevents_preparation_from_stale_discovery() {
    let (store, agent, parent) = fixture(AgentStatus::Ready).await;
    let admitted = admit(
        &store,
        &agent,
        &parent,
        "race",
        AgentCommand::message("one"),
    )
    .await;
    let (sender, inputs) = recording_sender(Ok(()));
    let operations = AgentOperationService::new(
        store.clone(),
        TestRuntimeRegistry::new(full_capabilities(), sender),
    );
    let execution = Execution::new(store.clone(), operations, Behavior::ClaimDuringCheck);
    execution
        .worker("worker")
        .run_one(&admitted.receipt.envelope.command_id)
        .await
        .unwrap();
    assert_eq!(execution.preparations.load(Ordering::SeqCst), 0);
    assert_eq!(execution.attempts.load(Ordering::SeqCst), 0);
    assert!(inputs.lock().unwrap().is_empty());
}

#[tokio::test]
async fn work_discovery_validates_lookahead_and_keeps_blocked_commands_in_stable_pages() {
    let (store, agent, parent) = fixture(AgentStatus::Paused).await;
    let a = admit(&store, &agent, &parent, "a", AgentCommand::message("a")).await;
    let b = admit(&store, &agent, &parent, "b", AgentCommand::message("b")).await;
    let first = store
        .command_work_page(&PageRequest::new(1, None).unwrap())
        .await
        .unwrap();
    assert_eq!(first.items.len(), 1);
    let second = store
        .command_work_page(&PageRequest::new(1, first.next.clone()).unwrap())
        .await
        .unwrap();
    assert_eq!(second.items.len(), 1);
    assert!(second.next.is_none());
    assert_ne!(first.items[0].receipt, second.items[0].receipt);
    let mut ordered = vec![a, b];
    ordered.sort_by(|a, b| {
        a.receipt
            .envelope
            .command_id
            .cmp(&b.receipt.envelope.command_id)
    });
    ordered[1].receipt.envelope.request_fingerprint = "bad".into();
    assert_eq!(
        finish_command_work_page(ordered, &PageRequest::new(1, None).unwrap()),
        Err(CommandLedgerError::Corrupt)
    );
    assert!(store.agent_log(&agent.id, 0, 10).await.unwrap().is_empty());
}
