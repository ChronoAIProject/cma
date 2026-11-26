use super::*;
use crate::ids::{AgentCommandId, AgentResponseId};

async fn fixture(prepared: bool) -> (Arc<InMemorySandboxStore>, Arc<Authority>, Agent) {
    if prepared {
        super::prepared::fixture().await
    } else {
        bootstrap_fixture().await
    }
}

struct RecordedReservations {
    inner: Arc<InMemorySandboxStore>,
    requests: Mutex<Vec<CommandReservation>>,
    refusal: Option<CommandLedgerError>,
    lost_reply: bool,
}

impl RecordedReservations {
    fn new(inner: Arc<InMemorySandboxStore>) -> Self {
        Self {
            inner,
            requests: Mutex::new(Vec::new()),
            refusal: None,
            lost_reply: false,
        }
    }
}

#[async_trait]
impl AgentCommandStore for RecordedReservations {
    async fn reserve_command(
        &self,
        request: CommandReservation,
    ) -> Result<CommandReservationResult, CommandLedgerError> {
        self.requests.lock().unwrap().push(request.clone());
        if let Some(error) = &self.refusal {
            return Err(error.clone());
        }
        let result = self.inner.reserve_command(request).await?;
        if self.lost_reply {
            Err(CommandLedgerError::Unavailable)
        } else {
            Ok(result)
        }
    }
    async fn get_command(
        &self,
        id: &AgentCommandId,
    ) -> Result<Option<AdmittedAgentCommand>, CommandLedgerError> {
        self.inner.get_command(id).await
    }
    async fn get_response_command(
        &self,
        id: &AgentResponseId,
    ) -> Result<Option<AdmittedAgentCommand>, CommandLedgerError> {
        self.inner.get_response_command(id).await
    }
    async fn get_admitted_command(
        &self,
        scope: &str,
        key_sha256: &str,
    ) -> Result<Option<AdmittedAgentCommand>, CommandLedgerError> {
        self.inner.get_admitted_command(scope, key_sha256).await
    }
}

#[tokio::test]
async fn creation_refresh_retains_request_identity_order_and_queue_expiry() {
    for prepared in [false, true] {
        let (store, authority, _) = fixture(prepared).await;
        let (_, progressing) = service(&store, authority.clone(), Progress::Parent);
        let recorded = Arc::new(RecordedReservations::new(store.clone()));
        let service = AgentCommandService::new(
            recorded.clone(),
            Arc::new(ResourceAccessService::new(store.clone(), authority)),
            progressing,
        );
        let status = service
            .admit(
                &context("owner"),
                "selected-credential",
                CommandSubmissionSource::Api,
                submission("retained"),
            )
            .await
            .unwrap();
        let requests = recorded.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        let before = &requests[0];
        let after = &requests[1];
        assert_eq!(before.envelope, after.envelope);
        assert_eq!(before.key_sha256, after.key_sha256);
        assert_eq!(before.response_id, after.response_id);
        assert_eq!(before.admitted_at, after.admitted_at);
        assert_eq!(before.queue_deadline, after.queue_deadline);
        assert!(after.parent.expected_revision > before.parent.expected_revision);
        assert_eq!(status.receipt.command_id, before.envelope.command_id);
        assert_eq!(status.receipt.response_id, before.response_id);
    }
}

#[tokio::test]
async fn creation_refresh_never_retries_ambiguous_commit_or_other_refusals() {
    for prepared in [false, true] {
        for refusal in [
            None,
            Some(CommandLedgerError::NotAdmissible),
            Some(CommandLedgerError::StaleClaim),
            Some(CommandLedgerError::QueueFull),
            Some(CommandLedgerError::Unavailable),
        ] {
            let (store, authority, _) = fixture(prepared).await;
            let lost_reply = refusal.is_none();
            let recorded = Arc::new(RecordedReservations {
                refusal: refusal.clone(),
                lost_reply,
                ..RecordedReservations::new(store.clone())
            });
            let service = AgentCommandService::new(
                recorded.clone(),
                Arc::new(ResourceAccessService::new(store.clone(), authority.clone())),
                authority.clone(),
            );
            let error = service
                .admit(
                    &context("owner"),
                    "selected-credential",
                    CommandSubmissionSource::Api,
                    submission("uncertain"),
                )
                .await
                .unwrap_err();
            let CommandAdmissionError::Ledger(error) = error else {
                panic!("expected ledger refusal");
            };
            assert_eq!(error, refusal.unwrap_or(CommandLedgerError::Unavailable));
            assert_eq!(recorded.requests.lock().unwrap().len(), 1);
            assert_eq!(authority.calls.load(Ordering::SeqCst), 1);
            let commands = store
                .command_work_page(&PageRequest::default())
                .await
                .unwrap()
                .items;
            assert_eq!(commands.len(), usize::from(lost_reply));
            if lost_reply {
                let original = recorded.requests.lock().unwrap()[0].clone();
                assert_eq!(
                    commands[0].receipt.envelope.command_id,
                    original.envelope.command_id
                );
                assert_eq!(commands[0].receipt.response_id, original.response_id);
                assert!(commands[0].work.dispatch.is_none());
            }
        }
    }
}

#[tokio::test(start_paused = true)]
async fn creation_refresh_cannot_extend_time_budget_or_change_idempotency_scope() {
    for prepared in [false, true] {
        for progress in [Progress::SlowRefresh, Progress::ChangedScope] {
            let (store, authority, _) = fixture(prepared).await;
            let (_, progressing) = service(&store, authority.clone(), progress);
            let recorded = Arc::new(RecordedReservations::new(store.clone()));
            let service = AgentCommandService::new(
                recorded.clone(),
                Arc::new(ResourceAccessService::new(store.clone(), authority)),
                progressing.clone(),
            );
            let started = tokio::time::Instant::now();
            let error = service
                .admit(
                    &context("owner"),
                    "selected-credential",
                    CommandSubmissionSource::Api,
                    submission("bounded"),
                )
                .await
                .unwrap_err();
            let CommandAdmissionError::Ledger(error) = error else {
                panic!("expected ledger refusal");
            };
            assert_eq!(
                error,
                if matches!(progress, Progress::SlowRefresh) {
                    CommandLedgerError::Unavailable
                } else {
                    CommandLedgerError::Conflict
                }
            );
            assert!(started.elapsed() <= Duration::from_secs(15));
            assert_eq!(recorded.requests.lock().unwrap().len(), 1);
            assert_eq!(progressing.calls.load(Ordering::SeqCst), 2);
            assert!(store
                .command_work_page(&PageRequest::default())
                .await
                .unwrap()
                .items
                .is_empty());
        }
    }
}

#[tokio::test]
async fn bootstrap_refresh_does_not_retry_generic_starting_agents_or_controls() {
    for bootstrap in [true, false] {
        let (store, authority) = if bootstrap {
            let (store, authority, _) = bootstrap_fixture().await;
            (store, authority)
        } else {
            let (store, _, authority, _) = setup(Authority::new()).await;
            (store, authority)
        };
        let (service, authority) = service(&store, authority, Progress::Parent);
        let mut input = submission("no-refresh");
        if bootstrap {
            input.command = AgentCommand::Interrupt;
        }
        assert!(matches!(
            service
                .admit(
                    &context("owner"),
                    "selected-credential",
                    CommandSubmissionSource::Api,
                    input
                )
                .await,
            Err(CommandAdmissionError::Ledger(CommandLedgerError::Conflict))
        ));
        assert_eq!(authority.calls.load(Ordering::SeqCst), 1);
        assert!(store
            .command_work_page(&PageRequest::default())
            .await
            .unwrap()
            .items
            .is_empty());
    }
}

async fn ordinary_fixture() -> (Arc<InMemorySandboxStore>, Arc<Authority>, Agent) {
    use crate::sandbox::SandboxHome;
    use layer3_adapter::{HomeVolumePolicy, HomeVolumeReceipt};

    let (store, _, authority, mut agent) = setup(Authority::new()).await;
    let revision = agent.revision;
    agent.status = AgentStatus::Ready;
    agent.ready_at = Some(NOW.into());
    agent.conversation.conversation_id = Some("ordinary-conversation".into());
    agent.conversation.runtime_generation = Some(3);
    agent.revision += 1;
    store.save_agent(revision, agent.clone()).await.unwrap();
    let mut parent = store.get_sandbox(&agent.sandbox_id).await.unwrap().unwrap();
    let revision = parent.revision;
    let mut home = SandboxHome::reserve(
        &parent.id,
        HomeVolumePolicy {
            mode: Default::default(),
            storage_class: "fixture-storage".into(),
            capacity_gib: 8,
            template_name: "fixture-home".into(),
            template_generation: "fixture-generation".into(),
        },
    )
    .unwrap();
    home.bind(HomeVolumeReceipt {
        native_authority: None,
        volume_id: home.request.volume_id.clone(),
        request_sha256: home.request.computed_sha256(),
        storage_reference: "fixture-volume".into(),
        storage_generation: "fixture-volume-generation".into(),
    })
    .unwrap();
    parent.lifecycle.home = Some(Box::new(home));
    parent.lifecycle.runtime_generation_high_water = 3;
    parent.runtime = Some(RuntimeBinding {
        boot_id: Some("a".repeat(32)),
        runtime_id: "runtime".into(),
        runtime_version: "v1".into(),
        provision_generation: Some(3),
        bound_at: NOW.into(),
    });
    parent.revision += 1;
    store.save_sandbox(revision, parent).await.unwrap();
    (store, authority, agent)
}

#[tokio::test]
async fn ordinary_ready_message_refreshes_a_known_aborted_revision_once() {
    let (store, authority, _) = ordinary_fixture().await;
    let (_, progressing) = service(&store, authority.clone(), Progress::Agent);
    let recorded = Arc::new(RecordedReservations::new(store.clone()));
    let service = AgentCommandService::new(
        recorded.clone(),
        Arc::new(ResourceAccessService::new(store.clone(), authority)),
        progressing.clone(),
    );
    let owner = context("owner");
    let request = submission("ordinary-ready-input");
    let accepted = service
        .admit(
            &owner,
            "selected-credential",
            CommandSubmissionSource::Api,
            request.clone(),
        )
        .await
        .unwrap();
    assert_eq!(progressing.calls.load(Ordering::SeqCst), 2);
    assert_eq!(accepted.status, AgentResponseStatus::Queued);
    {
        let requests = recorded.requests.lock().unwrap();
        let [first, refreshed] = requests.as_slice() else {
            panic!("one conflict and one refreshed attempt expected");
        };
        assert!(refreshed.expected_agent_revision > first.expected_agent_revision);
        assert_eq!(refreshed.parent, first.parent);
        assert_eq!(refreshed.envelope, first.envelope);
        assert_eq!(refreshed.key_sha256, first.key_sha256);
        assert_eq!(refreshed.response_id, first.response_id);
        assert_eq!(refreshed.queue_deadline, first.queue_deadline);
    }
    assert_eq!(
        service
            .lookup_admitted(
                &owner,
                "selected-credential",
                CommandSubmissionSource::Api,
                request
            )
            .await
            .unwrap(),
        Some(accepted)
    );
    assert_eq!(
        store
            .command_work_page(&PageRequest::default())
            .await
            .unwrap()
            .items
            .len(),
        1
    );
}

#[tokio::test]
async fn ordinary_ready_refresh_keeps_explicit_revision_and_refuses_changed_stop_authority() {
    for (progress, expected_explicit) in [(Progress::Agent, true), (Progress::Stop, false)] {
        let (store, authority, agent) = ordinary_fixture().await;
        let (_, progressing) = service(&store, authority.clone(), progress);
        let recorded = Arc::new(RecordedReservations::new(store.clone()));
        let service = AgentCommandService::new(
            recorded.clone(),
            Arc::new(ResourceAccessService::new(store.clone(), authority)),
            progressing.clone(),
        );
        let mut request = submission("ordinary-refused-input");
        request.expected_agent_revision = expected_explicit.then_some(agent.revision);
        assert!(matches!(
            service
                .admit(
                    &context("owner"),
                    "selected-credential",
                    CommandSubmissionSource::Api,
                    request
                )
                .await,
            Err(CommandAdmissionError::Ledger(CommandLedgerError::Conflict))
        ));
        assert_eq!(
            progressing.calls.load(Ordering::SeqCst),
            if expected_explicit { 1 } else { 2 }
        );
        assert_eq!(recorded.requests.lock().unwrap().len(), 1);
        assert!(store
            .command_work_page(&PageRequest::default())
            .await
            .unwrap()
            .items
            .is_empty());
    }
}

#[tokio::test]
async fn funded_ready_refresh_keeps_the_original_reservation_until_a_definitive_outcome() {
    use crate::sandbox::model_credit::{PhaseId, PhaseState, ReservationAuthority};
    use sha2::{Digest, Sha256};

    for (progress, should_admit) in [(Progress::Agent, true), (Progress::Stop, false)] {
        let (store, authority, _) = ordinary_fixture().await;
        let (_, progressing) = service(&store, authority.clone(), progress);
        let recorded = Arc::new(RecordedReservations::new(store.clone()));
        let (funding, ledger, payer) = super::super::execution::funding::funded_admission_fixture();
        let service = AgentCommandService::new(
            recorded.clone(),
            Arc::new(ResourceAccessService::new(store.clone(), authority)),
            progressing,
        )
        .with_model_credit_required(true)
        .with_model_funding(Some(funding));
        let result = service
            .admit(
                &context("owner"),
                "selected-credential",
                CommandSubmissionSource::Api,
                submission("ordinary-funded-input"),
            )
            .await;
        let requests = recorded.requests.lock().unwrap().clone();
        let first = &requests[0];
        let phase = PhaseId {
            turn: format!(
                "{:x}",
                Sha256::digest(format!(
                    "cma.model-turn.v1\n{}\n{}",
                    first.scope(),
                    first.key_sha256
                ))
            ),
            phase: 0,
        };
        if should_admit {
            let accepted = result.unwrap();
            assert_eq!(requests.len(), 2);
            let current = ledger.read(&phase).await.unwrap();
            assert_eq!(current.version.get(), 1);
            assert_eq!(current.state, PhaseState::Reserved);
            let identity = current.reservation.command.unwrap();
            assert_eq!(identity.command_id, accepted.receipt.command_id);
            assert_eq!(Some(identity.response_id), accepted.receipt.response_id);
            assert_eq!(requests[1].envelope.command_id, first.envelope.command_id);
            assert_eq!(*ledger.lock().unwrap().available(&payer), 0);
        } else {
            assert!(matches!(
                result,
                Err(CommandAdmissionError::Ledger(CommandLedgerError::Conflict))
            ));
            assert_eq!(requests.len(), 1);
            assert!(matches!(
                ledger.read(&phase).await,
                Err(crate::sandbox::model_credit::FundingError::NoReservation)
            ));
            assert_eq!(*ledger.lock().unwrap().available(&payer), 5);
        }
    }
}

#[tokio::test]
async fn cancelling_funded_ready_refresh_releases_the_known_aborted_participant() {
    let (store, mut authority, _) = ordinary_fixture().await;
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    Arc::get_mut(&mut authority).unwrap().gate = Some((entered.clone(), release.clone()));
    let (_, progressing) = service(&store, authority.clone(), Progress::Agent);
    let recorded = Arc::new(RecordedReservations::new(store.clone()));
    let (funding, ledger, payer) = super::super::execution::funding::funded_admission_fixture();
    let service = AgentCommandService::new(
        recorded.clone(),
        Arc::new(ResourceAccessService::new(store.clone(), authority)),
        progressing,
    )
    .with_model_credit_required(true)
    .with_model_funding(Some(funding));
    let task = tokio::spawn(async move {
        service
            .admit(
                &context("owner"),
                "selected-credential",
                CommandSubmissionSource::Api,
                submission("cancel-before-refresh"),
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), entered.notified())
        .await
        .unwrap();
    release.notify_one();
    tokio::time::timeout(Duration::from_secs(5), entered.notified())
        .await
        .unwrap();
    assert_eq!(recorded.requests.lock().unwrap().len(), 1);
    assert_eq!(*ledger.lock().unwrap().available(&payer), 0);
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    tokio::time::timeout(Duration::from_secs(5), async {
        while *ledger.lock().unwrap().available(&payer) != 5 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(store
        .command_work_page(&PageRequest::default())
        .await
        .unwrap()
        .items
        .is_empty());
}
