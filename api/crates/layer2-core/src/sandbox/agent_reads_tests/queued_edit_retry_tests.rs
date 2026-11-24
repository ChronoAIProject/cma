use super::*;
use crate::ids::{AgentCommandId, AgentResponseId};

async fn queued_edit_fixture() -> (
    Arc<InMemorySandboxStore>,
    Arc<Authority>,
    QueuedInputSubmission,
) {
    let (store, commands, authority, agent) = setup(Authority::new()).await;
    let owner = context("owner");
    let submitted = commands
        .admit(
            &owner,
            "selected-credential",
            CommandSubmissionSource::Api,
            submission("queued-edit-source"),
        )
        .await
        .unwrap();
    let edit = QueuedInputSubmission {
        agent_id: agent.id,
        target: QueuedCancellationTarget {
            response_id: submitted.receipt.response_id.unwrap(),
            source_command_id: submitted.receipt.command_id,
            expected_command_revision: 1,
        },
        idempotency_key: "queued-edit-intent".into(),
        text: "revised input".into(),
    };
    (store, authority, edit)
}

#[tokio::test]
async fn queued_edit_retries_agent_revision_progress_with_the_same_authority() {
    let (store, authority, edit) = queued_edit_fixture().await;
    let owner = context("owner");
    let (service, advancing_authority) = service(&store, authority, Progress::Agent);
    let saved = service
        .edit_queued_input(
            &owner,
            "selected-credential",
            CommandSubmissionSource::Api,
            edit.clone(),
        )
        .await
        .unwrap();
    assert_eq!(advancing_authority.calls.load(Ordering::SeqCst), 2);
    assert_eq!(saved.binding.text, edit.text);
    assert_eq!(saved.binding.command_revision, 2);
    let replayed = service
        .edit_queued_input(
            &owner,
            "selected-credential",
            CommandSubmissionSource::Api,
            edit,
        )
        .await
        .unwrap();
    assert_eq!(replayed, saved);
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
async fn queued_edit_refuses_changed_authority_and_limits_repreparation() {
    for progress in [Progress::ChangedScope, Progress::EveryAttempt] {
        let (store, authority, edit) = queued_edit_fixture().await;
        let before = store
            .get_command(&edit.target.source_command_id)
            .await
            .unwrap()
            .unwrap();
        let (service, advancing_authority) = service(&store, authority, progress);
        assert!(matches!(
            service
                .edit_queued_input(
                    &context("owner"),
                    "selected-credential",
                    CommandSubmissionSource::Api,
                    edit.clone(),
                )
                .await,
            Err(QueuedCancellationError::Admission(
                CommandAdmissionError::Ledger(CommandLedgerError::Conflict)
            ))
        ));
        assert_eq!(advancing_authority.calls.load(Ordering::SeqCst), 2);
        assert_eq!(
            store
                .get_command(&edit.target.source_command_id)
                .await
                .unwrap(),
            Some(before)
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
}

struct EditStore {
    inner: Arc<InMemorySandboxStore>,
    requests: Mutex<Vec<QueuedInputReservation>>,
    first_conflicts: bool,
}

#[async_trait]
impl AgentCommandStore for EditStore {
    async fn edit_queued_input(
        &self,
        request: QueuedInputReservation,
    ) -> Result<QueuedInputOutcome, CommandLedgerError> {
        let first = {
            let mut requests = self.requests.lock().unwrap();
            requests.push(request.clone());
            requests.len() == 1
        };
        if first && self.first_conflicts {
            return Err(CommandLedgerError::Conflict);
        }
        let result = self.inner.edit_queued_input(request).await?;
        if first {
            assert!(matches!(result, QueuedInputOutcome::Applied(_)));
            return Err(CommandLedgerError::Unavailable);
        }
        Ok(result)
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
async fn queued_edit_leaves_generic_conflicts_and_lost_replies_for_explicit_recovery() {
    for first_conflicts in [true, false] {
        let (store, authority, edit) = queued_edit_fixture().await;
        let edits = Arc::new(EditStore {
            inner: store.clone(),
            requests: Mutex::default(),
            first_conflicts,
        });
        let service = AgentCommandService::new(
            edits.clone(),
            Arc::new(ResourceAccessService::new(store.clone(), authority.clone())),
            authority,
        );
        let submit = || async {
            service
                .edit_queued_input(
                    &context("owner"),
                    "selected-credential",
                    CommandSubmissionSource::Api,
                    edit.clone(),
                )
                .await
        };
        let expected = if first_conflicts {
            CommandLedgerError::Conflict
        } else {
            CommandLedgerError::Unavailable
        };
        assert!(matches!(
            submit().await,
            Err(QueuedCancellationError::Admission(CommandAdmissionError::Ledger(error)))
                if error == expected
        ));
        let first_command = {
            let requests = edits.requests.lock().unwrap();
            assert_eq!(requests.len(), 1);
            requests[0].cancellation.command.clone()
        };
        let retained = store
            .get_admitted_command(&first_command.scope(), &first_command.key_sha256)
            .await
            .unwrap();
        let recovered = submit().await.unwrap();
        if first_conflicts {
            assert!(retained.is_none());
        } else {
            assert_eq!(
                recovered.edit.receipt.command_id,
                retained.unwrap().receipt.envelope.command_id
            );
        }
        assert_eq!(recovered.binding.command_revision, 2);
        assert_eq!(recovered.binding.text, edit.text);
        let source = store
            .get_command(&edit.target.source_command_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(source.revision, 2);
        assert_eq!(source.queued_input.unwrap().text, edit.text);
        assert_eq!(edits.requests.lock().unwrap().len(), 2);
    }
}
