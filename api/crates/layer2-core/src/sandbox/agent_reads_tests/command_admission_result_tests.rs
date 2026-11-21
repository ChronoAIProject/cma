use super::*;
use crate::ids::{AgentCommandId, AgentResponseId};
use crate::sandbox::AgentOperationError;

#[tokio::test]
async fn model_credit_refusal_never_reserves_a_new_key_and_preserves_exact_replay() {
    let (store, service, _, _) = setup(Authority::new()).await;
    let caller = context("owner");
    let source = CommandSubmissionSource::Api;
    let credential = "selected-credential";
    let accepted = service
        .admit(&caller, credential, source, submission("one"))
        .await
        .unwrap();
    let service = service.with_model_credit_required(true);
    let replay = service
        .admit(&caller, credential, source, submission("one"))
        .await
        .unwrap();
    assert_eq!(replay.receipt.command_id, accepted.receipt.command_id);
    assert!(matches!(
        service
            .admit(&caller, credential, source, submission("two"))
            .await,
        Err(CommandAdmissionError::Input(
            AgentOperationError::ModelFundingUnsupported
        ))
    ));
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

struct BufferedReservation {
    inner: Arc<InMemorySandboxStore>,
    entered: Arc<Notify>,
    release: Arc<Notify>,
    failed: bool,
}
#[async_trait]
impl AgentCommandStore for BufferedReservation {
    async fn reserve_command(
        &self,
        request: CommandReservation,
    ) -> Result<CommandReservationResult, CommandLedgerError> {
        let result = self.inner.reserve_command(request).await;
        self.entered.notify_one();
        self.release.notified().await;
        if self.failed {
            Err(CommandLedgerError::Unavailable)
        } else {
            result
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
async fn post_reservation_policy_change_refuses_reply_but_fresh_retry_recovers_same_receipt() {
    for failed in [false, true] {
        let (store, service, authority, agent) = setup(Authority::new()).await;
        let before_log = store.agent_log(&agent.id, 0, 20).await.unwrap();
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let buffered = AgentCommandService::new(
            Arc::new(BufferedReservation {
                inner: store.clone(),
                entered: entered.clone(),
                release: release.clone(),
                failed,
            }),
            Arc::new(ResourceAccessService::new(store.clone(), authority.clone())),
            authority,
        );
        let work = tokio::spawn(async move {
            buffered
                .admit(
                    &context("owner"),
                    "selected-credential",
                    CommandSubmissionSource::Api,
                    submission("one"),
                )
                .await
        });
        entered.notified().await;
        let original = store
            .command_work_page(&PageRequest::default())
            .await
            .unwrap()
            .items
            .remove(0);
        share(&store, None).await;
        release.notify_one();
        assert!(matches!(
            work.await.unwrap(),
            Err(CommandAdmissionError::Access(ResourceAccessError::Store(
                ResourceAccessStoreError::Conflict
            )))
        ));
        let recovered = service
            .admit(
                &context("owner"),
                "selected-credential",
                CommandSubmissionSource::Api,
                submission("one"),
            )
            .await
            .unwrap();
        assert_eq!(
            recovered.receipt.command_id,
            original.receipt.envelope.command_id
        );
        assert_eq!(recovered.receipt.response_id, original.receipt.response_id);
        assert_eq!(
            store
                .get_command(&recovered.receipt.command_id)
                .await
                .unwrap(),
            Some(original)
        );
        assert_eq!(store.agent_log(&agent.id, 0, 20).await.unwrap(), before_log);
        assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(agent));
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
}

#[tokio::test(start_paused = true)]
async fn lost_reservation_reply_times_out_without_losing_accepted_work_or_admitting_a_duplicate() {
    let (store, service, authority, _) = setup(Authority::new()).await;
    let entered = Arc::new(Notify::new());
    let buffered = AgentCommandService::new(
        Arc::new(BufferedReservation {
            inner: store.clone(),
            entered: entered.clone(),
            release: Arc::new(Notify::new()),
            failed: false,
        }),
        Arc::new(ResourceAccessService::new(store.clone(), authority.clone())),
        authority,
    );
    let work = tokio::spawn(async move {
        buffered
            .admit(
                &context("owner"),
                "selected-credential",
                CommandSubmissionSource::Api,
                submission("one"),
            )
            .await
    });
    entered.notified().await;
    tokio::time::advance(Duration::from_secs(15)).await;
    assert!(matches!(
        work.await.unwrap(),
        Err(CommandAdmissionError::Ledger(
            CommandLedgerError::Unavailable
        ))
    ));
    let original = store
        .command_work_page(&PageRequest::default())
        .await
        .unwrap()
        .items
        .remove(0);
    let recovered = service
        .admit(
            &context("owner"),
            "selected-credential",
            CommandSubmissionSource::Api,
            submission("one"),
        )
        .await
        .unwrap();
    assert_eq!(
        recovered.receipt.command_id,
        original.receipt.envelope.command_id
    );
    assert_eq!(recovered.receipt.response_id, original.receipt.response_id);
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
