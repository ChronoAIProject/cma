use super::*;

#[tokio::test]
async fn queue_timeout_is_terminal_without_checking_authority_or_starting_preparation() {
    use super::super::ledger_tests::{record, reservation};
    let (store, agent, parent) = fixture(AgentStatus::Paused).await;
    let original = record(
        store
            .reserve_command(reservation(&agent, &parent, "expired"))
            .await
            .unwrap(),
    );
    let (sender, inputs) = recording_sender(Ok(()));
    let operations = AgentOperationService::new(
        store.clone(),
        TestRuntimeRegistry::new(full_capabilities(), sender),
    );
    let execution = Execution::new(store.clone(), operations, Behavior::Execute);
    execution
        .worker("worker")
        .run_one(&original.receipt.envelope.command_id)
        .await
        .unwrap();
    let current = store
        .get_command(&original.receipt.envelope.command_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        current.work.failure.unwrap().code,
        CommandFailureCode::QueueTimeout
    );
    assert_eq!(current.receipt, original.receipt);
    assert_eq!(execution.checks.load(Ordering::SeqCst), 0);
    assert_eq!(execution.preparations.load(Ordering::SeqCst), 0);
    assert!(inputs.lock().unwrap().is_empty());
}

#[tokio::test]
async fn pause_after_claim_cannot_copy_a_durable_command_into_the_legacy_queue() {
    let (store, agent, parent) = fixture(AgentStatus::Ready).await;
    admit(
        &store,
        &agent,
        &parent,
        "pause-race",
        AgentCommand::message("one"),
    )
    .await;
    let claimed = store
        .claim_next_command(request(&agent, "worker", &crate::now_rfc3339()))
        .await
        .unwrap()
        .unwrap();
    let mut paused = agent.clone();
    paused.status = AgentStatus::Paused;
    paused.revision += 1;
    store
        .save_agent(agent.revision, paused.clone())
        .await
        .unwrap();
    let (sender, inputs) = recording_sender(Ok(()));
    let operations = AgentOperationService::new(
        store.clone(),
        TestRuntimeRegistry::new(full_capabilities(), sender),
    );
    assert!(operations
        .execute_claimed_command(&owner(), &claimed)
        .await
        .is_err());
    assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(paused));
    assert_eq!(
        store
            .get_command(&claimed.receipt.envelope.command_id)
            .await
            .unwrap(),
        Some(claimed)
    );
    assert!(inputs.lock().unwrap().is_empty());
    assert!(store.agent_log(&agent.id, 0, 10).await.unwrap().is_empty());
}
use crate::ids::AgentCommandId;
use crate::sandbox::store::Page;
use std::sync::atomic::AtomicBool;

struct UncertainStore {
    inner: Arc<InMemorySandboxStore>,
    fail_reads: AtomicBool,
    head_reads: AtomicUsize,
    head_timeout: AtomicBool,
    lose_renewal_reply: bool,
    releases: AtomicUsize,
}

#[async_trait::async_trait]
impl AgentCommandStore for UncertainStore {
    async fn get_response_command(
        &self,
        id: &crate::ids::AgentResponseId,
    ) -> Result<Option<AdmittedAgentCommand>, CommandLedgerError> {
        self.inner.get_response_command(id).await
    }
    async fn reserve_command(
        &self,
        request: CommandReservation,
    ) -> Result<CommandReservationResult, CommandLedgerError> {
        self.inner.reserve_command(request).await
    }
    async fn get_command(
        &self,
        id: &AgentCommandId,
    ) -> Result<Option<AdmittedAgentCommand>, CommandLedgerError> {
        if self.fail_reads.load(Ordering::SeqCst) {
            return Err(CommandLedgerError::Unavailable);
        }
        self.inner.get_command(id).await
    }
}

#[async_trait::async_trait]
impl AgentCommandWorkerStore for UncertainStore {
    async fn command_reconcile_heads(
        &self,
        agent_ids: &[crate::AgentId],
    ) -> Result<Vec<CommandReconcileHead>, CommandLedgerError> {
        self.head_reads.fetch_add(1, Ordering::SeqCst);
        if self.head_timeout.load(Ordering::SeqCst) {
            std::future::pending::<()>().await;
        }
        if self.fail_reads.load(Ordering::SeqCst) {
            return Err(CommandLedgerError::Unavailable);
        }
        self.inner.command_reconcile_heads(agent_ids).await
    }

    async fn command_work_page(
        &self,
        page: &PageRequest,
    ) -> Result<Page<AdmittedAgentCommand>, CommandLedgerError> {
        self.inner.command_work_page(page).await
    }
    async fn reject_unclaimed_command(
        &self,
        id: &AgentCommandId,
        revision: u64,
        code: CommandFailureCode,
        now: String,
    ) -> Result<(), CommandLedgerError> {
        self.inner
            .reject_unclaimed_command(id, revision, code, now)
            .await
    }
    async fn reconcile_command(
        &self,
        id: &AgentCommandId,
    ) -> Result<AdmittedAgentCommand, CommandLedgerError> {
        self.inner.reconcile_command(id).await
    }
    async fn claim_next_command(
        &self,
        request: CommandClaimRequest,
    ) -> Result<Option<AdmittedAgentCommand>, CommandLedgerError> {
        self.inner.claim_next_command(request).await
    }
    async fn claim_observed_command(
        &self,
        request: CommandClaimRequest,
        command_id: &AgentCommandId,
        expected_revision: u64,
    ) -> Result<Option<AdmittedAgentCommand>, CommandLedgerError> {
        self.inner
            .claim_observed_command(request, command_id, expected_revision)
            .await
    }
    async fn renew_command_claim(
        &self,
        claim: &CommandClaim,
        now: String,
        expires_at: String,
    ) -> Result<(), CommandLedgerError> {
        self.inner
            .renew_command_claim(claim, now, expires_at)
            .await?;
        if self.lose_renewal_reply {
            Err(CommandLedgerError::Unavailable)
        } else {
            Ok(())
        }
    }
    async fn release_command_claim(
        &self,
        claim: &CommandClaim,
        now: String,
    ) -> Result<(), CommandLedgerError> {
        self.releases.fetch_add(1, Ordering::SeqCst);
        self.inner.release_command_claim(claim, now).await
    }
    async fn reject_command(
        &self,
        claim: &CommandClaim,
        code: CommandFailureCode,
        now: String,
    ) -> Result<AdmittedAgentCommand, CommandLedgerError> {
        self.inner.reject_command(claim, code, now).await
    }
}

struct LostSnapshot {
    execution: Arc<Execution>,
    store: Arc<UncertainStore>,
}

#[async_trait::async_trait]
impl AgentCommandExecution for LostSnapshot {
    async fn check_queued(&self, command: &AdmittedAgentCommand) -> CommandReadiness {
        self.execution.check_queued(command).await
    }
    async fn prepare_queued(
        &self,
        command: &AdmittedAgentCommand,
    ) -> Result<(), CommandPreparationError> {
        self.execution.prepare_queued(command).await
    }
    async fn execute(&self, command: &AdmittedAgentCommand) -> CommandAttempt {
        self.execution.execute(command).await;
        self.store.fail_reads.store(true, Ordering::SeqCst);
        CommandAttempt::Reject(CommandFailureCode::RuntimeUnavailable)
    }
}

#[tokio::test]
async fn failed_settlement_read_cannot_establish_absence_of_dispatch() {
    let (store, agent, parent) = fixture(AgentStatus::Ready).await;
    let admitted = admit(
        &store,
        &agent,
        &parent,
        "lost-snapshot",
        AgentCommand::message("one"),
    )
    .await;
    let (sender, inputs) = recording_sender(Ok(()));
    let operations = AgentOperationService::new(
        store.clone(),
        TestRuntimeRegistry::new(full_capabilities(), sender),
    );
    let execution = Execution::new(store.clone(), operations, Behavior::Execute);
    let uncertain = Arc::new(UncertainStore {
        inner: store.clone(),
        fail_reads: AtomicBool::new(false),
        head_reads: AtomicUsize::new(0),
        head_timeout: AtomicBool::new(false),
        lose_renewal_reply: false,
        releases: AtomicUsize::new(0),
    });
    let lost = Arc::new(LostSnapshot {
        execution: execution.clone(),
        store: uncertain.clone(),
    });
    let worker = AgentCommandWorker::new(uncertain.clone(), lost, "worker".into()).unwrap();
    let id = &admitted.receipt.envelope.command_id;
    assert_eq!(
        worker.run_one(id).await,
        Err(CommandLedgerError::Unavailable)
    );
    assert_eq!(uncertain.releases.load(Ordering::SeqCst), 0);
    let bound = store.get_command(id).await.unwrap().unwrap();
    assert!(bound.work.dispatch.is_some());
    assert!(bound.work.failure.is_none());
    assert!(bound.work.lease.is_some());
    uncertain.fail_reads.store(false, Ordering::SeqCst);
    worker.run_one(id).await.unwrap();
    assert_eq!(execution.attempts.load(Ordering::SeqCst), 1);
    assert_eq!(inputs.lock().unwrap().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn lost_renewal_reply_stops_execution_and_rereads_before_releasing_unbound_work() {
    let (store, agent, parent) = fixture(AgentStatus::Ready).await;
    let admitted = admit(
        &store,
        &agent,
        &parent,
        "lost-renewal",
        AgentCommand::message("one"),
    )
    .await;
    let (sender, inputs) = recording_sender(Ok(()));
    let operations = AgentOperationService::new(
        store.clone(),
        TestRuntimeRegistry::new(full_capabilities(), sender),
    );
    let execution = Execution::new(store.clone(), operations, Behavior::HangBeforeBinding);
    let uncertain = Arc::new(UncertainStore {
        inner: store.clone(),
        fail_reads: AtomicBool::new(false),
        head_reads: AtomicUsize::new(0),
        head_timeout: AtomicBool::new(false),
        lose_renewal_reply: true,
        releases: AtomicUsize::new(0),
    });
    let worker = AgentCommandWorker::new(uncertain.clone(), execution, "worker".into()).unwrap();
    worker
        .run_one(&admitted.receipt.envelope.command_id)
        .await
        .unwrap();
    assert_eq!(uncertain.releases.load(Ordering::SeqCst), 1);
    let current = store
        .get_command(&admitted.receipt.envelope.command_id)
        .await
        .unwrap()
        .unwrap();
    assert!(current.work.dispatch.is_none());
    assert!(current.work.lease.is_none());
    assert!(current.work.failure.is_none());
    assert!(inputs.lock().unwrap().is_empty());
}

#[tokio::test]
async fn personal_command_cannot_bind_an_organization_with_the_same_owner_subject() {
    use crate::resource_access::*;
    let (store, agent, parent) = fixture(AgentStatus::Ready).await;
    admit(
        &store,
        &agent,
        &parent,
        "owner-kind",
        AgentCommand::message("one"),
    )
    .await;
    let claimed = store
        .claim_next_command(request(&agent, "worker", &crate::now_rfc3339()))
        .await
        .unwrap()
        .unwrap();
    store.set_resource_policy_for_test(ResourcePolicyRecord {
        sandbox_id: parent.id.clone(),
        sandbox_created_at: parent.created_at.clone(),
        owner: ResourceOwner::Organization {
            subject: parent.owner.clone(),
        },
        revision: 1,
        organization_evidence: Some(OrganizationOwnershipEvidence {
            issuer: "https://identity.test".into(),
            organization: parent.owner.clone(),
            admitted_by: ResourceActor::Human {
                subject: crate::SubjectId::new("admin"),
            },
            verified_at: NOW.into(),
        }),
    });
    let (sender, inputs) = recording_sender(Ok(()));
    let operations = AgentOperationService::new(
        store.clone(),
        TestRuntimeRegistry::new(full_capabilities(), sender),
    );
    assert_eq!(
        operations.execute_claimed_command(&owner(), &claimed).await,
        Err(AgentOperationError::Conflict)
    );
    assert!(inputs.lock().unwrap().is_empty());
    assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(agent));
    assert_eq!(
        store
            .get_command(&claimed.receipt.envelope.command_id)
            .await
            .unwrap(),
        Some(claimed)
    );
}

#[path = "command_driver_pacing_tests.rs"]
mod pacing;
