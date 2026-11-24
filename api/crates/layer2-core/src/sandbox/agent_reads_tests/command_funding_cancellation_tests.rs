use super::*;
use crate::sandbox::model_credit::ReservationAuthority;

#[derive(Default)]
struct PendingGateway {
    permit: Mutex<Option<Checkpoint>>,
    finality: Mutex<Option<Effect>>,
    entered: Notify,
    dropped: AtomicBool,
    sends: AtomicUsize,
}

struct ObserveDrop<'a>(&'a AtomicBool);

impl Drop for ObserveDrop<'_> {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

#[async_trait]
impl AgentModelGateway for PendingGateway {
    async fn selected_credential(
        &self,
        _: &Agent,
        _: &Sandbox,
    ) -> Result<CredentialBinding, FundingError> {
        Ok(credential(CredentialSource::Managed))
    }

    async fn first_phase_limit(
        &self,
        _: &CredentialBinding,
        _: &AgentCommand,
    ) -> Result<NonZeroU64, FundingError> {
        Ok(units(5))
    }
}

#[async_trait]
impl FundedModelGateway for PendingGateway {
    type Request = ModelPhaseRequest;

    async fn certify(&self, selected: &CredentialBinding) -> Result<PayerBinding, FundingError> {
        Ok(payer(selected.clone()))
    }

    fn digest(&self, request: &ModelPhaseRequest) -> [u8; 32] {
        Sha256::digest(request.command.fingerprint().unwrap()).into()
    }

    async fn execute(
        &self,
        permit: &Checkpoint,
        request: ModelPhaseRequest,
    ) -> Result<Effect, FundingError> {
        assert_eq!(permit.state, PhaseState::InFlight);
        assert_eq!(request.target.unwrap().runtime_generation, 3);
        assert_eq!(request.command, AgentCommand::message("private input"));
        assert_eq!(self.sends.fetch_add(1, Ordering::SeqCst), 0);
        *self.permit.lock().unwrap() = Some(permit.clone());
        let _drop = ObserveDrop(&self.dropped);
        self.entered.notify_one();
        std::future::pending().await
    }

    async fn reconcile(&self, permit: &Checkpoint) -> Result<Effect, FundingError> {
        assert_eq!(self.permit.lock().unwrap().as_ref(), Some(permit));
        self.finality
            .lock()
            .unwrap()
            .ok_or(FundingError::ReconciliationRequired)
    }
}

struct FundedExecution {
    access: Arc<ResourceAccessService>,
    operations: Arc<AgentOperationService>,
}

#[async_trait]
impl AgentCommandExecution for FundedExecution {
    async fn check_queued(&self, _: &AdmittedAgentCommand) -> CommandReadiness {
        CommandReadiness::Ready
    }

    async fn prepare_queued(
        &self,
        _: &AdmittedAgentCommand,
    ) -> Result<(), CommandPreparationError> {
        panic!("the fixture Agent is ready")
    }

    async fn execute(&self, command: &AdmittedAgentCommand) -> CommandAttempt {
        let authorization = self
            .access
            .authorize_command_execution(
                &context("owner"),
                "selected-credential",
                &command.receipt.envelope,
            )
            .await
            .unwrap();
        let result = self
            .operations
            .execute_resource_claim(&authorization, command)
            .await;
        panic!("the pending gateway must be cancelled, not return: {result:?}")
    }
}

#[derive(Clone, Copy)]
enum WorkerExit {
    AttemptTimeout,
    ClaimLost,
    TaskAborted,
}

async fn cancelled_worker_preserves_funding(exit: WorkerExit) {
    let (store, access, authority, agent) = execution_fixture(MAX_MEMBERSHIP_AGE).await;
    let binding = payer(credential(CredentialSource::Managed));
    let ledger = Arc::new(Ledger::default());
    ledger.lock().unwrap().add(&binding, 5);
    let gateway = Arc::new(PendingGateway::default());
    let funding = AgentTurnFunding::new(ledger.clone(), gateway.clone());
    let access = Arc::new(access);
    let commands = AgentCommandService::new(store.clone(), access.clone(), authority)
        .with_model_credit_required(true)
        .with_model_funding(Some(funding.clone()));
    let admitted = commands
        .admit(
            &context("owner"),
            "selected-credential",
            CommandSubmissionSource::Api,
            submission("cancelled-worker"),
        )
        .await
        .unwrap();
    let id = admitted.receipt.command_id.clone();
    let original = store.get_command(&id).await.unwrap().unwrap();
    let (registry, legacy_sends) = runtime(store.clone(), None, 0);
    let operations = AgentOperationService::new(store.clone(), registry)
        .with_model_credit_required(true)
        .with_model_funding(Some(funding.clone()));
    let worker = AgentCommandWorker::new(
        store.clone(),
        Arc::new(FundedExecution { access, operations }),
        "original".into(),
    )
    .unwrap()
    .with_model_funding(Some(funding));
    let run_id = id.clone();
    let started = tokio::time::Instant::now();
    let task = tokio::spawn(async move { worker.run_scheduled(&run_id).await });
    tokio::time::timeout(Duration::from_secs(1), gateway.entered.notified())
        .await
        .expect("the gateway must receive the funded request");
    let sent = store.get_command(&id).await.unwrap().unwrap();
    let permit = gateway.permit.lock().unwrap().clone().unwrap();
    assert_eq!(permit.reservation.command.as_ref().unwrap().command_id, id);
    assert_eq!(sent.receipt, original.receipt);
    assert!(sent.work.dispatch.is_some());
    assert!(!gateway.dropped.load(Ordering::SeqCst));
    match exit {
        WorkerExit::AttemptTimeout => {
            tokio::time::advance(Duration::from_secs(44)).await;
            assert!(!task.is_finished());
            task.await.unwrap().unwrap();
            assert_eq!(started.elapsed(), Duration::from_secs(45));
        }
        WorkerExit::ClaimLost => {
            store
                .release_command_claim(
                    &sent.work.lease.as_ref().unwrap().claim,
                    crate::now_rfc3339(),
                )
                .await
                .unwrap();
            task.await.unwrap().unwrap();
            assert_eq!(started.elapsed(), Duration::from_secs(10));
        }
        WorkerExit::TaskAborted => {
            task.abort();
            assert!(task.await.unwrap_err().is_cancelled());
            assert_eq!(started.elapsed(), Duration::ZERO);
        }
    }
    drop(commands);
    assert!(gateway.dropped.load(Ordering::SeqCst));
    assert_eq!(ledger.read(&permit.reservation.id).await.unwrap(), permit);
    assert_eq!(*ledger.lock().unwrap().available(&binding), 0);

    // Product cancellation is independent of the synthetic gateway's finality.
    DurableAgentUpdateRecorder::new(store.clone())
        .record_update(
            &agent.id,
            3,
            &mut AgentMessageAssembly::default(),
            layer3_adapter::ConversationUpdate::TurnEnded(layer3_adapter::PromptResponse::new(
                layer3_adapter::TurnStopReason::Cancelled,
            )),
            crate::now_rfc3339(),
        )
        .await
        .unwrap();
    let cancelled = store.reconcile_command(&id).await.unwrap();
    assert_eq!(cancelled.status, AgentResponseStatus::Cancelled);
    let page = PageRequest::new(1, None).unwrap();
    assert!(store
        .command_work_page(&page)
        .await
        .unwrap()
        .items
        .is_empty());
    let successor =
        AgentCommandWorker::new(store.clone(), Arc::new(NoRedispatch), "successor".into())
            .unwrap()
            .with_model_funding(Some(AgentTurnFunding::new(ledger.clone(), gateway.clone())));
    for _ in 0..2 {
        let pending = successor.discover_scheduled(&page).await.unwrap();
        assert_eq!(pending.page.items.len(), 1);
        assert_eq!(pending.page.items[0].id, id);
        assert!(!successor.run_scheduled(&id).await.unwrap().is_zero());
        assert_eq!(ledger.read(&permit.reservation.id).await.unwrap(), permit);
        assert_eq!(*ledger.lock().unwrap().available(&binding), 0);
    }
    *gateway.finality.lock().unwrap() = Some(Effect::Finished { spent: 3 });
    for _ in 0..2 {
        assert!(successor.run_scheduled(&id).await.unwrap().is_zero());
        assert_eq!(*ledger.lock().unwrap().available(&binding), 2);
        assert_eq!(
            store.get_command(&id).await.unwrap(),
            Some(cancelled.clone())
        );
    }
    assert!(successor
        .discover_scheduled(&page)
        .await
        .unwrap()
        .page
        .items
        .is_empty());
    assert_eq!(
        ledger.read(&permit.reservation.id).await.unwrap().state,
        PhaseState::Settled(Effect::Finished { spent: 3 })
    );
    assert_eq!(gateway.sends.load(Ordering::SeqCst), 1);
    assert_eq!(legacy_sends.load(Ordering::SeqCst), 0);
}

#[tokio::test(start_paused = true)]
async fn model_credit_worker_attempt_timeout_retains_inflight_funding() {
    cancelled_worker_preserves_funding(WorkerExit::AttemptTimeout).await;
}

#[tokio::test(start_paused = true)]
async fn model_credit_worker_claim_loss_retains_inflight_funding() {
    cancelled_worker_preserves_funding(WorkerExit::ClaimLost).await;
}

#[tokio::test(start_paused = true)]
async fn model_credit_worker_task_abort_retains_inflight_funding() {
    cancelled_worker_preserves_funding(WorkerExit::TaskAborted).await;
}
