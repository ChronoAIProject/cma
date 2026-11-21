use super::*;

enum FirstWrite {
    UnavailableBeforeCommit,
    LoseCommittedReply,
    Delay {
        entered: Arc<Notify>,
        proceed: Arc<Notify>,
        definite_refusal: bool,
    },
}

struct AdmissionStore {
    store: Arc<InMemorySandboxStore>,
    first: Mutex<Option<FirstWrite>>,
}

impl AdmissionStore {
    fn new(store: Arc<InMemorySandboxStore>, first: FirstWrite) -> Arc<Self> {
        Arc::new(Self {
            store,
            first: Mutex::new(Some(first)),
        })
    }
}

#[async_trait]
impl AgentCommandStore for AdmissionStore {
    async fn reserve_command(
        &self,
        request: CommandReservation,
    ) -> Result<CommandReservationResult, CommandLedgerError> {
        let first = self.first.lock().unwrap().take();
        match first {
            Some(FirstWrite::UnavailableBeforeCommit) => {
                return Err(CommandLedgerError::Unavailable)
            }
            Some(FirstWrite::LoseCommittedReply) => {
                self.store.reserve_command(request).await?;
                return Err(CommandLedgerError::Unavailable);
            }
            Some(FirstWrite::Delay {
                entered,
                proceed,
                definite_refusal,
            }) => {
                entered.notify_one();
                proceed.notified().await;
                if definite_refusal {
                    return Err(CommandLedgerError::Conflict);
                }
            }
            None => {}
        }
        self.store.reserve_command(request).await
    }

    async fn get_admitted_command(
        &self,
        scope: &str,
        key_sha256: &str,
    ) -> Result<Option<AdmittedAgentCommand>, CommandLedgerError> {
        self.store.get_admitted_command(scope, key_sha256).await
    }

    async fn get_command(
        &self,
        id: &AgentCommandId,
    ) -> Result<Option<AdmittedAgentCommand>, CommandLedgerError> {
        self.store.get_command(id).await
    }

    async fn get_response_command(
        &self,
        id: &AgentResponseId,
    ) -> Result<Option<AdmittedAgentCommand>, CommandLedgerError> {
        self.store.get_response_command(id).await
    }
}

#[tokio::test]
async fn model_credit_lost_admission_reply_retains_exact_committed_hold() {
    let (store, access, authority, _) = execution_fixture(MAX_MEMBERSHIP_AGE).await;
    let ledger = Arc::new(Ledger::default());
    let payer = payer(credential(CredentialSource::Managed));
    ledger.lock().unwrap().add(&payer, 5);
    let funding = AgentTurnFunding::new(ledger.clone(), gateway(false));
    let commands = AgentCommandService::new(
        AdmissionStore::new(store.clone(), FirstWrite::LoseCommittedReply),
        Arc::new(access),
        authority,
    )
    .with_model_credit_required(true)
    .with_model_funding(Some(funding.clone()));
    let caller = context("owner");
    let admit = || {
        commands.admit(
            &caller,
            "selected-credential",
            CommandSubmissionSource::Api,
            submission("unknown-admission"),
        )
    };
    assert!(matches!(
        admit().await,
        Err(CommandAdmissionError::Ledger(
            CommandLedgerError::Unavailable
        ))
    ));
    let replay = admit().await.unwrap();
    let page = PageRequest::new(1, None).unwrap();
    assert_eq!(
        funding.pending_commands(&page).await.unwrap().items,
        vec![replay.receipt.command_id]
    );
    assert_eq!(*ledger.lock().unwrap().available(&payer), 0);
}

#[tokio::test]
async fn model_credit_exact_retry_recovers_pending_identity_after_unavailable_admission() {
    let (store, access, authority, agent) = execution_fixture(MAX_MEMBERSHIP_AGE).await;
    let ledger = Arc::new(Ledger::default());
    let payer = payer(credential(CredentialSource::Managed));
    ledger.lock().unwrap().add(&payer, 5);
    let gateway = gateway(false);
    let funding = AgentTurnFunding::new(ledger.clone(), gateway.clone());
    let commands = AgentCommandService::new(
        AdmissionStore::new(store.clone(), FirstWrite::UnavailableBeforeCommit),
        Arc::new(access),
        authority,
    )
    .with_model_credit_required(true)
    .with_model_funding(Some(funding.clone()));
    let caller = context("owner");
    let admit = |request| {
        commands.admit(
            &caller,
            "selected-credential",
            CommandSubmissionSource::Api,
            request,
        )
    };
    let key = "recover-unavailable-command";
    assert!(matches!(
        admit(submission(key)).await,
        Err(CommandAdmissionError::Ledger(
            CommandLedgerError::Unavailable
        ))
    ));
    let page = PageRequest::new(1, None).unwrap();
    let pending = funding.pending_commands(&page).await.unwrap();
    let id = &pending.items[0];
    assert!(store.get_command(id).await.unwrap().is_none());
    let worker = AgentCommandWorker::new(
        store.clone(),
        Arc::new(NoRedispatch),
        "recovered-worker".into(),
    )
    .unwrap()
    .with_model_funding(Some(funding.clone()));
    assert!(!worker.run_scheduled(id).await.unwrap().is_zero());
    assert_eq!(*ledger.lock().unwrap().available(&payer), 0);

    let mut stale = submission(key);
    stale.expected_agent_revision = Some(agent.revision + 1);
    assert!(matches!(
        admit(stale).await,
        Err(CommandAdmissionError::Ledger(CommandLedgerError::Conflict))
    ));
    assert_eq!(*ledger.lock().unwrap().available(&payer), 0);
    assert_eq!(
        funding.pending_commands(&page).await.unwrap().items,
        pending.items
    );

    let mut changed = submission(key);
    changed.command = AgentCommand::message("a different request");
    assert!(matches!(
        admit(changed).await,
        Err(CommandAdmissionError::Input(
            AgentOperationError::ModelFunding(FundingError::CheckpointChanged)
        ))
    ));
    gateway.credential_generation.store(2, Ordering::SeqCst);
    assert!(matches!(
        admit(submission(key)).await,
        Err(CommandAdmissionError::Input(
            AgentOperationError::ModelFunding(FundingError::BindingChanged)
        ))
    ));
    gateway.credential_generation.store(1, Ordering::SeqCst);

    let recovered = admit(submission(key)).await.unwrap();
    assert_eq!(&recovered.receipt.command_id, id);
    assert_eq!(
        admit(submission(key)).await.unwrap().receipt,
        recovered.receipt
    );
    assert_eq!(store.command_work_page(&page).await.unwrap().items.len(), 1);
    assert_eq!(*ledger.lock().unwrap().available(&payer), 0);
    assert_eq!(gateway.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn model_credit_delayed_admission_cannot_release_recovered_winners_hold() {
    for definite_refusal in [false, true] {
        let (store, access, authority, _) = execution_fixture(MAX_MEMBERSHIP_AGE).await;
        let ledger = Arc::new(Ledger::default());
        let payer = payer(credential(CredentialSource::Managed));
        ledger.lock().unwrap().add(&payer, 5);
        let gateway = gateway(false);
        let funding = AgentTurnFunding::new(ledger.clone(), gateway.clone());
        let entered = Arc::new(Notify::new());
        let proceed = Arc::new(Notify::new());
        let interrupted = AdmissionStore::new(
            store.clone(),
            FirstWrite::Delay {
                entered: entered.clone(),
                proceed: proceed.clone(),
                definite_refusal,
            },
        );
        let access = Arc::new(access);
        let first = AgentCommandService::new(interrupted, access.clone(), authority.clone())
            .with_model_credit_required(true)
            .with_model_funding(Some(funding.clone()));
        let retry = AgentCommandService::new(store.clone(), access, authority)
            .with_model_credit_required(true)
            .with_model_funding(Some(funding.clone()));
        let caller = context("owner");
        let submit = |commands: Arc<AgentCommandService>, caller: CallerContext| async move {
            commands
                .admit(
                    &caller,
                    "selected-credential",
                    CommandSubmissionSource::Api,
                    submission("delayed-original"),
                )
                .await
        };
        let first = tokio::spawn(submit(first, caller.clone()));
        tokio::time::timeout(Duration::from_secs(1), entered.notified())
            .await
            .unwrap();
        let page = PageRequest::new(1, None).unwrap();
        let pending = funding.pending_commands(&page).await.unwrap();
        assert!(store
            .get_command(&pending.items[0])
            .await
            .unwrap()
            .is_none());
        let recovered = submit(retry, caller).await.unwrap();
        assert_eq!(recovered.receipt.command_id, pending.items[0]);
        proceed.notify_one();
        let initial = first.await.unwrap();
        if definite_refusal {
            assert!(matches!(
                initial,
                Err(CommandAdmissionError::Ledger(CommandLedgerError::Conflict))
            ));
        } else {
            assert_eq!(initial.unwrap().receipt, recovered.receipt);
        }
        let work = store.command_work_page(&page).await.unwrap();
        assert_eq!(work.items.len(), 1);
        assert!(work.next.is_none());
        assert_eq!(
            funding.pending_commands(&page).await.unwrap().items,
            pending.items
        );
        assert_eq!(*ledger.lock().unwrap().available(&payer), 0);
        assert_eq!(gateway.calls.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn model_credit_two_definitive_refusals_return_the_balance_after_both_writers_finish() {
    let (store, access, authority, agent) = execution_fixture(MAX_MEMBERSHIP_AGE).await;
    let ledger = Arc::new(Ledger::default());
    let payer = payer(credential(CredentialSource::Managed));
    ledger.lock().unwrap().add(&payer, 5);
    let gateway = gateway(false);
    let funding = AgentTurnFunding::new(ledger.clone(), gateway.clone());
    let entered = Arc::new(Notify::new());
    let proceed = Arc::new(Notify::new());
    let delayed = AdmissionStore::new(
        store.clone(),
        FirstWrite::Delay {
            entered: entered.clone(),
            proceed: proceed.clone(),
            definite_refusal: false,
        },
    );
    let access = Arc::new(access);
    let first = AgentCommandService::new(delayed, access.clone(), authority.clone())
        .with_model_credit_required(true)
        .with_model_funding(Some(funding.clone()));
    let retry = AgentCommandService::new(store.clone(), access, authority)
        .with_model_credit_required(true)
        .with_model_funding(Some(funding.clone()));
    let revision = agent.revision;
    let submit = move |commands: Arc<AgentCommandService>| async move {
        let mut request = submission("two-definitive-refusals");
        request.expected_agent_revision = Some(revision);
        commands
            .admit(
                &context("owner"),
                "selected-credential",
                CommandSubmissionSource::Api,
                request,
            )
            .await
    };
    let first = tokio::spawn(submit(first));
    tokio::time::timeout(Duration::from_secs(2), entered.notified())
        .await
        .unwrap();

    let mut changed = agent;
    changed.revision += 1;
    store.save_agent(revision, changed).await.unwrap();
    assert!(matches!(
        submit(retry).await,
        Err(CommandAdmissionError::Ledger(CommandLedgerError::Conflict))
    ));
    assert_eq!(*ledger.lock().unwrap().available(&payer), 0);
    let page = PageRequest::new(1, None).unwrap();
    let pending = funding.pending_commands(&page).await.unwrap();
    assert_eq!(pending.items.len(), 1);
    proceed.notify_one();
    assert!(matches!(
        first.await.unwrap(),
        Err(CommandAdmissionError::Ledger(CommandLedgerError::Conflict))
    ));

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
    assert_eq!(gateway.calls.load(Ordering::SeqCst), 0);
}
