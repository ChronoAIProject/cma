use super::*;
use crate::sandbox::model_credit::FundedAgentMessage;

#[derive(Clone, Copy, Debug)]
enum CredentialChange {
    Rotation,
    SelectionUnavailable,
    CertificationUnavailable,
}

struct CompetingRuntime {
    runtime: Arc<Runtime>,
    store: Arc<InMemorySandboxStore>,
    command: AdmittedAgentCommand,
    funding: Arc<AgentTurnFunding>,
    gateway: Arc<Gateway>,
    change: CredentialChange,
    dispatched: AtomicBool,
}

#[async_trait]
impl AgentRuntimeRegistry for CompetingRuntime {
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
            && !self.dispatched.swap(true, Ordering::SeqCst)
        {
            let sandbox = self
                .store
                .get_sandbox(&agent.sandbox_id)
                .await
                .unwrap()
                .unwrap();
            let message = FundedAgentMessage::admitted(&self.command).unwrap();
            assert_eq!(
                self.funding
                    .dispatch_message(&message, &agent, &sandbox, target.clone())
                    .await,
                Err(FundingError::ReconciliationRequired)
            );
            match self.change {
                CredentialChange::Rotation => self
                    .gateway
                    .credential_generation
                    .store(2, Ordering::SeqCst),
                CredentialChange::SelectionUnavailable => self
                    .gateway
                    .selection_unavailable
                    .store(true, Ordering::SeqCst),
                CredentialChange::CertificationUnavailable => self
                    .gateway
                    .certification_unavailable
                    .store(true, Ordering::SeqCst),
            }
        }
        self.runtime.resolve(target).await
    }

    async fn fence(&self, agent: &AgentId, generation: u64) -> Result<(), AgentRuntimeError> {
        self.runtime.fence(agent, generation).await
    }
}

#[tokio::test]
async fn model_credit_in_flight_work_survives_credential_changes_before_dispatch_recheck() {
    for change in [
        CredentialChange::Rotation,
        CredentialChange::SelectionUnavailable,
        CredentialChange::CertificationUnavailable,
    ] {
        let (store, access, authority, agent) = execution_fixture(MAX_MEMBERSHIP_AGE).await;
        let binding = payer(credential(CredentialSource::Managed));
        let ledger = Arc::new(Ledger::default());
        ledger.lock().unwrap().add(&binding, 5);
        let gateway = gateway(true);
        let funding = AgentTurnFunding::new(ledger.clone(), gateway.clone());
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
                submission("funding-binding-race"),
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
        let registry = Arc::new(CompetingRuntime {
            runtime,
            store: store.clone(),
            command: command.clone(),
            funding: funding.clone(),
            gateway: gateway.clone(),
            change,
            dispatched: AtomicBool::new(false),
        });
        let operations = AgentOperationService::new(store.clone(), registry)
            .with_model_credit_required(true)
            .with_model_funding(Some(funding.clone()));
        let outcome = operations
            .execute_resource_claim(&authorization, &command)
            .await;
        assert_eq!(gateway.calls.load(Ordering::SeqCst), 1);
        let current = store.get_agent(&agent.id).await.unwrap().unwrap();
        assert_eq!(
            current.conversation.operation.as_ref().unwrap().state,
            AgentOperationDelivery::RecoveryRequired,
            "{change:?}: {outcome:?}"
        );
        assert!(matches!(
            outcome,
            Err(AgentOperationError::DeliveryAmbiguous { .. })
        ));
        assert!(current.conversation.active_turn.is_some());
        let message = FundedAgentMessage::admitted(&command).unwrap();
        let sandbox = store.get_sandbox(&agent.sandbox_id).await.unwrap().unwrap();
        assert_eq!(
            funding.check_message(&message, &current, &sandbox).await,
            Err(FundingError::ReconciliationRequired)
        );
        let mut foreign = command.clone();
        foreign.receipt.envelope.command_id = AgentCommandId::generate();
        let foreign = FundedAgentMessage::admitted(&foreign).unwrap();
        assert_eq!(
            funding.check_message(&foreign, &current, &sandbox).await,
            Err(FundingError::CheckpointChanged)
        );
        assert_eq!(*ledger.lock().unwrap().available(&binding), 0);
        let id = &command.receipt.envelope.command_id;
        let retained = store.get_command(id).await.unwrap().unwrap();
        assert_eq!(retained.receipt, command.receipt);
        assert!(retained.work.dispatch.is_some());
        let successor = AgentCommandWorker::new(store, Arc::new(NoRedispatch), "successor".into())
            .unwrap()
            .with_model_funding(Some(AgentTurnFunding::new(ledger.clone(), gateway.clone())));
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
}
