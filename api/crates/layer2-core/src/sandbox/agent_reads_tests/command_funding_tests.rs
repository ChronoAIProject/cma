use super::*;
use crate::sandbox::agent_io::{
    AgentCommandExecution, AgentCommandWorker, CommandAttempt, CommandFailureCode,
    CommandPreparationError, CommandReadiness,
};
use crate::sandbox::model_credit::{
    tests::{credential, payer, units, Ledger},
    AgentModelGateway, AgentTurnFunding, Checkpoint, CredentialBinding, CredentialSource, Effect,
    FundedModelGateway, FundingError, ModelPhaseRequest, PayerBinding, PhaseState,
};
use crate::sandbox::{AgentMessageAssembly, DurableAgentUpdateRecorder};
use sha2::{Digest, Sha256};
use std::num::NonZeroU64;
use std::sync::atomic::AtomicBool;

struct Gateway {
    lose_reply: bool,
    selection_unavailable: AtomicBool,
    certification_unavailable: AtomicBool,
    verified_effect: AtomicBool,
    lookup_unavailable: AtomicBool,
    credential_generation: std::sync::atomic::AtomicU64,
    calls: AtomicUsize,
}

#[async_trait]
impl AgentModelGateway for Gateway {
    async fn selected_credential(
        &self,
        _: &Agent,
        _: &Sandbox,
    ) -> Result<CredentialBinding, FundingError> {
        if self.selection_unavailable.load(Ordering::SeqCst) {
            return Err(FundingError::Unavailable);
        }
        Ok(CredentialBinding {
            generation: units(self.credential_generation.load(Ordering::SeqCst)),
            ..credential(CredentialSource::Managed)
        })
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
        if self.certification_unavailable.load(Ordering::SeqCst) {
            return Err(FundingError::Unavailable);
        }
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
        if self.lookup_unavailable.load(Ordering::SeqCst) {
            return Err(FundingError::Unavailable);
        }
        self.verified_effect
            .load(Ordering::SeqCst)
            .then_some(Effect::Finished { spent: 3 })
            .ok_or(FundingError::ReconciliationRequired)
    }
}

fn gateway(lose_reply: bool) -> Arc<Gateway> {
    Arc::new(Gateway {
        lose_reply,
        selection_unavailable: AtomicBool::new(false),
        certification_unavailable: AtomicBool::new(false),
        verified_effect: AtomicBool::new(false),
        lookup_unavailable: AtomicBool::new(false),
        credential_generation: std::sync::atomic::AtomicU64::new(1),
        calls: AtomicUsize::new(0),
    })
}

pub(in crate::sandbox::agent_reads_tests::command_admission_tests) fn funded_admission_fixture(
) -> (Arc<AgentTurnFunding>, Arc<Ledger>, PayerBinding) {
    let payer = payer(credential(CredentialSource::Managed));
    let ledger = Arc::new(Ledger::default());
    ledger.lock().unwrap().add(&payer, 5);
    (
        AgentTurnFunding::new(ledger.clone(), gateway(false)),
        ledger,
        payer,
    )
}

struct NoRedispatch;

#[async_trait]
impl AgentCommandExecution for NoRedispatch {
    async fn check_queued(&self, _: &AdmittedAgentCommand) -> CommandReadiness {
        panic!("command was already dispatched or terminated")
    }

    async fn prepare_queued(
        &self,
        _: &AdmittedAgentCommand,
    ) -> Result<(), CommandPreparationError> {
        panic!("command was already dispatched or terminated")
    }

    async fn execute(&self, _: &AdmittedAgentCommand) -> CommandAttempt {
        panic!("recovery must not dispatch again")
    }
}

#[tokio::test]
async fn model_credit_application_dispatch_and_recovery_share_the_admitted_phase() {
    for (lose_reply, terminal_first) in [(false, false), (true, false), (true, true)] {
        let (store, access, authority, agent) = execution_fixture(MAX_MEMBERSHIP_AGE).await;
        let payer = payer(credential(CredentialSource::Managed));
        let ledger = Arc::new(Ledger::default());
        let gateway = gateway(lose_reply);
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
        let successor = AgentCommandWorker::new(
            store.clone(),
            Arc::new(NoRedispatch),
            "second-worker".into(),
        )
        .unwrap()
        .with_model_funding(Some(AgentTurnFunding::new(ledger.clone(), gateway.clone())));
        let id = &command.receipt.envelope.command_id;
        let page = PageRequest::new(1, None).unwrap();
        if lose_reply {
            gateway.lookup_unavailable.store(true, Ordering::SeqCst);
            assert!(!successor.run_scheduled(id).await.unwrap().is_zero());
            assert_eq!(*ledger.lock().unwrap().available(&payer), 0);
            if terminal_first {
                DurableAgentUpdateRecorder::new(store.clone())
                    .record_update(
                        &agent.id,
                        3,
                        &mut AgentMessageAssembly::default(),
                        layer3_adapter::ConversationUpdate::TurnEnded(
                            layer3_adapter::PromptResponse::new(
                                layer3_adapter::TurnStopReason::EndTurn,
                            ),
                        ),
                        crate::now_rfc3339(),
                    )
                    .await
                    .unwrap();
                assert!(store
                    .reconcile_command(id)
                    .await
                    .unwrap()
                    .status
                    .is_terminal());
                assert!(store
                    .command_work_page(&page)
                    .await
                    .unwrap()
                    .items
                    .is_empty());
            }
            let discovered = successor.discover_scheduled(&page).await.unwrap();
            assert_eq!(discovered.page.items.len(), 1);
            assert_eq!(&discovered.page.items[0].id, id);
            gateway.lookup_unavailable.store(false, Ordering::SeqCst);
        }
        successor.run_scheduled(id).await.unwrap();
        successor.run_scheduled(id).await.unwrap();
        if terminal_first {
            assert!(successor
                .discover_scheduled(&page)
                .await
                .unwrap()
                .page
                .items
                .is_empty());
        }
        assert_eq!(*ledger.lock().unwrap().available(&payer), 2);
        assert_eq!(gateway.calls.load(Ordering::SeqCst), 1);
        assert_eq!(legacy_sends.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn model_credit_releases_definitive_refusal_and_keeps_the_same_key_winner_funded() {
    let (store, access, authority, agent) = execution_fixture(MAX_MEMBERSHIP_AGE).await;
    let payer = payer(credential(CredentialSource::Managed));
    let ledger = Arc::new(Ledger::default());
    let gateway = gateway(false);
    let funding = AgentTurnFunding::new(ledger.clone(), gateway.clone());
    let commands = AgentCommandService::new(store.clone(), Arc::new(access), authority)
        .with_model_credit_required(true)
        .with_model_funding(Some(funding.clone()));
    let caller = context("owner");
    ledger.lock().unwrap().add(&payer, 5);
    let mut stale = submission("reusable-funding-key");
    stale.expected_agent_revision = Some(agent.revision + 1);
    assert!(matches!(
        commands
            .admit(
                &caller,
                "selected-credential",
                CommandSubmissionSource::Api,
                stale
            )
            .await,
        Err(CommandAdmissionError::Ledger(CommandLedgerError::Conflict))
    ));
    let page = PageRequest::new(50, None).unwrap();
    assert!(store
        .command_work_page(&page)
        .await
        .unwrap()
        .items
        .is_empty());
    assert!(funding
        .pending_commands(&page)
        .await
        .unwrap()
        .items
        .is_empty());
    assert_eq!(*ledger.lock().unwrap().available(&payer), 5);

    let admit = || {
        commands.admit(
            &caller,
            "selected-credential",
            CommandSubmissionSource::Api,
            submission("reusable-funding-key"),
        )
    };
    let (left, right) = tokio::join!(admit(), admit());
    let replay = admit().await.unwrap();
    for result in [left, right] {
        assert_eq!(result.unwrap().receipt, replay.receipt);
    }
    let id = &replay.receipt.command_id;
    assert_eq!(
        funding.pending_commands(&page).await.unwrap().items,
        vec![id.clone()]
    );
    assert_eq!(*ledger.lock().unwrap().available(&payer), 0);

    let command = store.get_command(id).await.unwrap().unwrap();
    store
        .reject_unclaimed_command(
            id,
            command.revision,
            CommandFailureCode::QueueTimeout,
            command.queue_deadline,
        )
        .await
        .unwrap();
    assert!(store
        .command_work_page(&page)
        .await
        .unwrap()
        .items
        .is_empty());
    let worker = AgentCommandWorker::new(store, Arc::new(NoRedispatch), "successor".into())
        .unwrap()
        .with_model_funding(Some(funding));
    assert_eq!(
        &worker.discover_scheduled(&page).await.unwrap().page.items[0].id,
        id
    );
    worker.run_scheduled(id).await.unwrap();
    assert!(worker
        .discover_scheduled(&page)
        .await
        .unwrap()
        .page
        .items
        .is_empty());
    assert_eq!(*ledger.lock().unwrap().available(&payer), 5);
    assert_eq!(gateway.calls.load(Ordering::SeqCst), 0);
}

#[path = "command_funding_recovery_tests.rs"]
mod admission_recovery;

#[path = "command_funding_dispatch_tests.rs"]
mod dispatch_recovery;

#[path = "command_funding_binding_tests.rs"]
mod binding_recovery;

#[path = "command_funding_check_race_tests.rs"]
mod check_races;
