use super::*;
use crate::sandbox::model_credit::FundedAgentMessage;
use std::sync::Mutex;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FailedCheck {
    Read,
    Selection,
    Certification,
    Retirement,
    UncontestedSelection,
    RuntimeResolution,
    RuntimeCapability,
    UncontestedRuntimeResolution,
    UncontestedRuntimeCapability,
}

impl FailedCheck {
    fn uncontested(self) -> bool {
        matches!(
            self,
            Self::UncontestedSelection
                | Self::UncontestedRuntimeResolution
                | Self::UncontestedRuntimeCapability
        )
    }
}

struct Competition {
    funding: Arc<AgentTurnFunding>,
    message: FundedAgentMessage,
    agent: Agent,
    sandbox: Sandbox,
    target: AgentRuntimeTarget,
}

impl Competition {
    async fn execute(self) {
        assert_eq!(
            self.funding
                .dispatch_message(&self.message, &self.agent, &self.sandbox, self.target)
                .await,
            Err(FundingError::ReconciliationRequired)
        );
    }
}

struct CheckedGateway {
    gateway: Arc<Gateway>,
    check: FailedCheck,
    competition: Mutex<Option<Competition>>,
}

impl CheckedGateway {
    async fn compete(&self, check: FailedCheck) -> bool {
        if self.check != check {
            return false;
        }
        let competition = self.competition.lock().unwrap().take();
        if let Some(competition) = competition {
            competition.execute().await;
            true
        } else {
            false
        }
    }
}

#[async_trait]
impl AgentModelGateway for CheckedGateway {
    async fn selected_credential(
        &self,
        agent: &Agent,
        sandbox: &Sandbox,
    ) -> Result<CredentialBinding, FundingError> {
        if self.compete(FailedCheck::Selection).await {
            return Err(FundingError::Unavailable);
        }
        self.gateway.selected_credential(agent, sandbox).await
    }

    async fn first_phase_limit(
        &self,
        selected: &CredentialBinding,
        request: &AgentCommand,
    ) -> Result<NonZeroU64, FundingError> {
        self.gateway.first_phase_limit(selected, request).await
    }
}

#[async_trait]
impl FundedModelGateway for CheckedGateway {
    type Request = ModelPhaseRequest;

    async fn certify(&self, selected: &CredentialBinding) -> Result<PayerBinding, FundingError> {
        if self.compete(FailedCheck::Certification).await {
            return Err(FundingError::BindingChanged);
        }
        self.gateway.certify(selected).await
    }

    fn digest(&self, request: &ModelPhaseRequest) -> [u8; 32] {
        self.gateway.digest(request)
    }

    async fn execute(
        &self,
        permit: &Checkpoint,
        request: ModelPhaseRequest,
    ) -> Result<Effect, FundingError> {
        self.gateway.execute(permit, request).await
    }

    async fn reconcile(&self, permit: &Checkpoint) -> Result<Effect, FundingError> {
        self.gateway.reconcile(permit).await
    }
}

struct CheckRuntime {
    runtime: Arc<Runtime>,
    store: Arc<InMemorySandboxStore>,
    command: AdmittedAgentCommand,
    ledger: Arc<Ledger>,
    checked: Arc<CheckedGateway>,
    armed: AtomicBool,
    target: Mutex<Option<AgentRuntimeTarget>>,
}

#[async_trait]
impl AgentRuntimeRegistry for CheckRuntime {
    async fn resolve(
        &self,
        target: &AgentRuntimeTarget,
    ) -> Result<AgentRuntimeConnection, AgentRuntimeError> {
        let agent = self
            .store
            .get_agent(&target.agent_id)
            .await
            .unwrap()
            .unwrap();
        if agent
            .conversation
            .operation
            .as_ref()
            .is_some_and(|operation| operation.state == AgentOperationDelivery::DispatchFenced)
            && !self.armed.swap(true, Ordering::SeqCst)
        {
            *self.target.lock().unwrap() = Some(target.clone());
            let sandbox = self
                .store
                .get_sandbox(&agent.sandbox_id)
                .await
                .unwrap()
                .unwrap();
            let competition = Competition {
                funding: AgentTurnFunding::new(self.ledger.clone(), self.checked.gateway.clone()),
                message: FundedAgentMessage::admitted(&self.command).unwrap(),
                agent,
                sandbox,
                target: target.clone(),
            };
            if matches!(
                self.checked.check,
                FailedCheck::RuntimeResolution
                    | FailedCheck::RuntimeCapability
                    | FailedCheck::UncontestedRuntimeResolution
                    | FailedCheck::UncontestedRuntimeCapability
            ) {
                if !self.checked.check.uncontested() {
                    competition.execute().await;
                }
                if matches!(
                    self.checked.check,
                    FailedCheck::RuntimeResolution | FailedCheck::UncontestedRuntimeResolution
                ) {
                    return Err(AgentRuntimeError::Unavailable(
                        "runtime lookup unavailable after dispatch was fenced".into(),
                    ));
                }
                let mut connection = self.runtime.resolve(target).await?;
                connection.capabilities = RuntimeCapabilities::new([]);
                return Ok(connection);
            }
            if matches!(
                self.checked.check,
                FailedCheck::UncontestedSelection | FailedCheck::Retirement
            ) {
                self.checked
                    .gateway
                    .selection_unavailable
                    .store(true, Ordering::SeqCst);
                if self.checked.check == FailedCheck::Retirement {
                    *self.checked.competition.lock().unwrap() = Some(competition);
                }
            } else if self.checked.check == FailedCheck::Read {
                competition.execute().await;
                self.ledger.lock().unwrap().read_unavailable = true;
            } else {
                *self.checked.competition.lock().unwrap() = Some(competition);
            }
        }
        self.runtime.resolve(target).await
    }

    async fn fence(&self, agent: &AgentId, generation: u64) -> Result<(), AgentRuntimeError> {
        self.runtime.fence(agent, generation).await
    }
}

async fn failed_check_preserves_competing_dispatch(check: FailedCheck) {
    let (store, access, authority, agent) = execution_fixture(MAX_MEMBERSHIP_AGE).await;
    let binding = payer(credential(CredentialSource::Managed));
    let ledger = Arc::new(Ledger::default());
    ledger.lock().unwrap().add(&binding, 5);
    let gateway = gateway(true);
    let checked = Arc::new(CheckedGateway {
        gateway: gateway.clone(),
        check,
        competition: Mutex::new(None),
    });
    let funding = AgentTurnFunding::new(ledger.clone(), checked.clone());
    let access = Arc::new(access);
    let commands = AgentCommandService::new(store.clone(), access.clone(), authority)
        .with_model_credit_required(true)
        .with_model_funding(Some(funding.clone()));
    let caller = context("owner");
    commands
        .admit(
            &caller,
            "selected-credential",
            CommandSubmissionSource::Api,
            submission("funding-check-race"),
        )
        .await
        .unwrap();
    let now = SystemTime::now();
    let command = store
        .claim_next_command(CommandClaimRequest {
            agent_id: agent.id.clone(),
            holder: "worker".into(),
            now: crate::rfc3339_at(now),
            expires_at: crate::rfc3339_at(now + Duration::from_secs(30)),
        })
        .await
        .unwrap()
        .unwrap();
    let authorization = access
        .authorize_command_execution(&caller, "selected-credential", &command.receipt.envelope)
        .await
        .unwrap();
    let (runtime, legacy_sends) = runtime(store.clone(), None, 0);
    let registry = Arc::new(CheckRuntime {
        runtime,
        store: store.clone(),
        command: command.clone(),
        ledger: ledger.clone(),
        checked,
        armed: AtomicBool::new(false),
        target: Mutex::new(None),
    });
    let operations = AgentOperationService::new(store.clone(), registry.clone())
        .with_model_credit_required(true)
        .with_model_funding(Some(funding.clone()));
    let execution = operations.execute_resource_claim(&authorization, &command);
    let outcome = if check == FailedCheck::Retirement {
        let barrier = Arc::new(tokio::sync::Barrier::new(2));
        ledger.lock().unwrap().settle_barrier = Some(barrier.clone());
        let competing = async {
            barrier.wait().await;
            gateway.selection_unavailable.store(false, Ordering::SeqCst);
            let competition = registry.checked.competition.lock().unwrap().take().unwrap();
            competition.execute().await;
            barrier.wait().await;
        };
        tokio::time::timeout(Duration::from_secs(5), async {
            tokio::join!(execution, competing).0
        })
        .await
        .expect("dispatch and retirement must both finish")
    } else {
        execution.await
    };
    let current = store.get_agent(&agent.id).await.unwrap().unwrap();
    if check.uncontested() {
        assert_eq!(gateway.calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            current.conversation.operation.as_ref().unwrap().state,
            AgentOperationDelivery::PreWriteFailed
        );
        assert!(current.conversation.active_turn.is_none());
        assert_eq!(
            *ledger.lock().unwrap().available(&binding),
            5,
            "rollback must have a committed never-started funding outcome"
        );
        gateway.selection_unavailable.store(false, Ordering::SeqCst);
        let message = FundedAgentMessage::admitted(&command).unwrap();
        let sandbox = store.get_sandbox(&agent.sandbox_id).await.unwrap().unwrap();
        let target = registry.target.lock().unwrap().clone().unwrap();
        assert_eq!(
            funding
                .dispatch_message(&message, &current, &sandbox, target)
                .await,
            Ok(Effect::NotStarted)
        );
        assert_eq!(gateway.calls.load(Ordering::SeqCst), 0);
        assert_eq!(legacy_sends.load(Ordering::SeqCst), 0);
        assert_eq!(*ledger.lock().unwrap().available(&binding), 5);
        return;
    }
    assert_eq!(gateway.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        current.conversation.operation.as_ref().unwrap().state,
        AgentOperationDelivery::RecoveryRequired,
        "{check:?}: {outcome:?}"
    );
    assert!(matches!(
        outcome,
        Err(AgentOperationError::DeliveryAmbiguous { .. })
    ));
    assert!(current.conversation.active_turn.is_some());
    let id = &command.receipt.envelope.command_id;
    let retained = store.get_command(id).await.unwrap().unwrap();
    assert_eq!(retained.receipt, command.receipt);
    assert!(retained.work.dispatch.is_some());
    assert_eq!(*ledger.lock().unwrap().available(&binding), 0);
    let successor = AgentCommandWorker::new(store, Arc::new(NoRedispatch), "successor".into())
        .unwrap()
        .with_model_funding(Some(AgentTurnFunding::new(ledger.clone(), gateway.clone())));
    if check == FailedCheck::Read {
        assert!(!successor.run_scheduled(id).await.unwrap().is_zero());
        assert_eq!(*ledger.lock().unwrap().available(&binding), 0);
    }
    ledger.lock().unwrap().read_unavailable = false;
    gateway.lookup_unavailable.store(true, Ordering::SeqCst);
    assert!(!successor.run_scheduled(id).await.unwrap().is_zero());
    assert_eq!(*ledger.lock().unwrap().available(&binding), 0);
    gateway.lookup_unavailable.store(false, Ordering::SeqCst);
    successor.run_scheduled(id).await.unwrap();
    successor.run_scheduled(id).await.unwrap();
    assert_eq!(*ledger.lock().unwrap().available(&binding), 2);
    assert_eq!(gateway.calls.load(Ordering::SeqCst), 1);
    assert_eq!(legacy_sends.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn model_credit_read_failure_after_competing_dispatch_preserves_recovery() {
    failed_check_preserves_competing_dispatch(FailedCheck::Read).await;
}

#[tokio::test]
async fn model_credit_selection_failure_after_competing_dispatch_preserves_recovery() {
    failed_check_preserves_competing_dispatch(FailedCheck::Selection).await;
}

#[tokio::test]
async fn model_credit_certification_failure_after_competing_dispatch_preserves_recovery() {
    failed_check_preserves_competing_dispatch(FailedCheck::Certification).await;
}

#[tokio::test]
async fn model_credit_failed_check_retires_an_unstarted_phase_before_rollback() {
    failed_check_preserves_competing_dispatch(FailedCheck::UncontestedSelection).await;
}

#[tokio::test]
async fn model_credit_dispatch_winning_retirement_race_preserves_recovery() {
    failed_check_preserves_competing_dispatch(FailedCheck::Retirement).await;
}

#[tokio::test]
async fn model_credit_runtime_resolution_failure_preserves_competing_dispatch() {
    failed_check_preserves_competing_dispatch(FailedCheck::RuntimeResolution).await;
}

#[tokio::test]
async fn model_credit_runtime_capability_failure_preserves_competing_dispatch() {
    failed_check_preserves_competing_dispatch(FailedCheck::RuntimeCapability).await;
}

#[tokio::test]
async fn model_credit_runtime_resolution_failure_retires_unstarted_funding() {
    failed_check_preserves_competing_dispatch(FailedCheck::UncontestedRuntimeResolution).await;
}

#[tokio::test]
async fn model_credit_runtime_capability_failure_retires_unstarted_funding() {
    failed_check_preserves_competing_dispatch(FailedCheck::UncontestedRuntimeCapability).await;
}
