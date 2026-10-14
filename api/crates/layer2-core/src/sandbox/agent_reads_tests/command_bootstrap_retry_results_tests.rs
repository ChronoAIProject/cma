use super::*;
use crate::ids::{AgentCommandId, AgentResponseId};

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
}

#[tokio::test]
async fn bootstrap_refresh_retains_request_identity_order_and_queue_expiry() {
    let (store, authority, _) = bootstrap_fixture().await;
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

#[tokio::test]
async fn bootstrap_refresh_never_retries_ambiguous_commit_or_other_refusals() {
    for refusal in [
        None,
        Some(CommandLedgerError::NotAdmissible),
        Some(CommandLedgerError::StaleClaim),
        Some(CommandLedgerError::QueueFull),
        Some(CommandLedgerError::Unavailable),
    ] {
        let (store, authority, _) = bootstrap_fixture().await;
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

#[tokio::test(start_paused = true)]
async fn bootstrap_refresh_cannot_extend_time_budget_or_change_idempotency_scope() {
    for progress in [Progress::SlowRefresh, Progress::ChangedScope] {
        let (store, authority, _) = bootstrap_fixture().await;
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
