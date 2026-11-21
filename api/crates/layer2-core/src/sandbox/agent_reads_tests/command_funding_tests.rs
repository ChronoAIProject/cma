use super::*;
use crate::sandbox::model_credit::{
    tests::{credential, payer, units, Ledger},
    AgentModelGateway, AgentTurnFunding, Checkpoint, CredentialBinding, CredentialSource, Effect,
    FundedModelGateway, FundingError, ModelPhaseRequest, PayerBinding, PhaseState,
};
use sha2::{Digest, Sha256};
use std::num::NonZeroU64;
use std::sync::atomic::AtomicBool;

struct Gateway {
    lose_reply: bool,
    verified_effect: AtomicBool,
    calls: AtomicUsize,
}

#[async_trait]
impl AgentModelGateway for Gateway {
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
impl FundedModelGateway for Gateway {
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
        assert_eq!(permit.reservation.limit, units(5));
        assert_eq!(self.calls.fetch_add(1, Ordering::SeqCst), 0);
        self.verified_effect.store(true, Ordering::SeqCst);
        if self.lose_reply {
            return Err(FundingError::Unavailable);
        }
        Ok(Effect::Finished { spent: 3 })
    }

    async fn reconcile(&self, _: &Checkpoint) -> Result<Effect, FundingError> {
        self.verified_effect
            .load(Ordering::SeqCst)
            .then_some(Effect::Finished { spent: 3 })
            .ok_or(FundingError::ReconciliationRequired)
    }
}

#[tokio::test]
async fn model_credit_application_dispatch_and_recovery_share_the_admitted_phase() {
    for lose_reply in [false, true] {
        let (store, access, authority, agent) = execution_fixture(MAX_MEMBERSHIP_AGE).await;
        let payer = payer(credential(CredentialSource::Managed));
        let ledger = Arc::new(Ledger::default());
        let gateway = Arc::new(Gateway {
            lose_reply,
            verified_effect: AtomicBool::new(false),
            calls: AtomicUsize::new(0),
        });
        let funding = AgentTurnFunding::new(ledger.clone(), gateway.clone());
        let access = Arc::new(access);
        let commands = AgentCommandService::new(store.clone(), access.clone(), authority)
            .with_model_credit_required(true)
            .with_model_funding(Some(funding.clone()));
        let caller = context("owner");
        let admit = |key| {
            commands.admit(
                &caller,
                "selected-credential",
                CommandSubmissionSource::Api,
                submission(key),
            )
        };
        assert!(matches!(
            admit("funded-turn").await,
            Err(CommandAdmissionError::Input(
                AgentOperationError::ModelFunding(FundingError::Insufficient)
            ))
        ));
        assert!(store
            .command_work_page(&PageRequest::default())
            .await
            .unwrap()
            .items
            .is_empty());
        ledger.lock().unwrap().add(&payer, 5);
        let accepted = admit("funded-turn").await.unwrap();
        let replay = admit("funded-turn").await.unwrap();
        assert_eq!(accepted.receipt.command_id, replay.receipt.command_id);
        assert_eq!(*ledger.lock().unwrap().available(&payer), 0);

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
        let (registry, legacy_sends) = runtime(store.clone(), None, 0);
        let operations = AgentOperationService::new(store.clone(), registry)
            .with_model_credit_required(true)
            .with_model_funding(Some(funding));
        let dispatched = operations
            .execute_resource_claim(&authorization, &command)
            .await;
        if lose_reply {
            assert!(matches!(
                dispatched,
                Err(AgentOperationError::DeliveryAmbiguous { .. })
            ));
            assert_eq!(*ledger.lock().unwrap().available(&payer), 0);
        } else {
            assert!(matches!(
                dispatched,
                Ok(crate::sandbox::AgentMessageOutcome::Dispatched(_))
            ));
        }
        let successor = AgentTurnFunding::new(ledger.clone(), gateway.clone());
        let reconciled = successor.reconcile_message(&command).await.unwrap();
        assert_eq!(
            reconciled.state,
            PhaseState::Settled(Effect::Finished { spent: 3 })
        );
        assert_eq!(
            successor.reconcile_message(&command).await.unwrap(),
            reconciled
        );
        assert_eq!(*ledger.lock().unwrap().available(&payer), 2);
        assert_eq!(gateway.calls.load(Ordering::SeqCst), 1);
        assert_eq!(legacy_sends.load(Ordering::SeqCst), 0);
    }
}
