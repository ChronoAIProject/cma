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
async fn queued_edit_recovers_the_saved_intent_after_two_stale_observations() {
    let (store, authority, edit) = queued_edit_fixture().await;
    let edits = Arc::new(EditStore {
        inner: store.clone(),
        requests: Mutex::default(),
        first_reply: FirstEditReply::Conflict,
    });
    let (_, advancing_authority) = service(&store, authority.clone(), Progress::TwoAgentRevisions);
    let access = Arc::new(ResourceAccessService::new(store.clone(), authority.clone()));
    let save = AgentCommandService::new(edits.clone(), access.clone(), authority);
    let reload = AgentCommandService::new(edits.clone(), access, advancing_authority.clone());
    let submit = |service: Arc<AgentCommandService>| {
        let edit = edit.clone();
        async move {
            service
                .edit_queued_input(
                    &context("owner"),
                    "selected-credential",
                    CommandSubmissionSource::Api,
                    edit,
                )
                .await
        }
    };
    assert!(matches!(
        submit(save).await,
        Err(QueuedCancellationError::Admission(
            CommandAdmissionError::Ledger(CommandLedgerError::Conflict)
        ))
    ));
    let saved = submit(reload.clone()).await.unwrap();
    assert_eq!(advancing_authority.calls.load(Ordering::SeqCst), 3);
    assert_eq!(saved.binding.command_revision, 2);
    assert_eq!(submit(reload).await.unwrap(), saved);
    let source = store
        .get_command(&edit.target.source_command_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(source.revision, 2);
    assert_eq!(source.queued_input.unwrap().text, edit.text);
    let (scope, key) = {
        let requests = edits.requests.lock().unwrap();
        assert_eq!(requests.len(), 5);
        let first = &requests[0].cancellation.command;
        assert!(requests.iter().all(|request| {
            request.text == edit.text
                && request.cancellation.target == edit.target
                && request.cancellation.command.key_sha256 == first.key_sha256
        }));
        (first.scope(), first.key_sha256.clone())
    };
    let retained = store
        .get_admitted_command(&scope, &key)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        retained.receipt.envelope.command_id,
        saved.edit.receipt.command_id
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
        let expected_calls = if matches!(progress, Progress::EveryAttempt) {
            3
        } else {
            2
        };
        assert_eq!(
            advancing_authority.calls.load(Ordering::SeqCst),
            expected_calls
        );
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

#[derive(Clone, Copy)]
enum FirstEditReply {
    Conflict,
    Lost,
    Delayed,
    StaleTwiceThenDelayed,
}

struct EditStore {
    inner: Arc<InMemorySandboxStore>,
    requests: Mutex<Vec<QueuedInputReservation>>,
    first_reply: FirstEditReply,
}

#[async_trait]
impl AgentCommandStore for EditStore {
    async fn edit_queued_input(
        &self,
        request: QueuedInputReservation,
    ) -> Result<QueuedInputOutcome, CommandLedgerError> {
        let attempt = {
            let mut requests = self.requests.lock().unwrap();
            requests.push(request.clone());
            requests.len()
        };
        let first = attempt == 1;
        if first && matches!(self.first_reply, FirstEditReply::Conflict) {
            return Err(CommandLedgerError::Conflict);
        }
        if matches!(self.first_reply, FirstEditReply::StaleTwiceThenDelayed) {
            if attempt <= 2 {
                return Ok(QueuedInputOutcome::StaleObservation);
            }
            tokio::time::sleep(Duration::from_secs(6)).await;
        }
        let result = self.inner.edit_queued_input(request).await?;
        if first {
            assert!(matches!(result, QueuedInputOutcome::Applied(_)));
            match self.first_reply {
                FirstEditReply::Conflict | FirstEditReply::Lost => {
                    return Err(CommandLedgerError::Unavailable);
                }
                FirstEditReply::Delayed => tokio::time::sleep(Duration::from_secs(10)).await,
                FirstEditReply::StaleTwiceThenDelayed => unreachable!(),
            }
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

struct EditDeadlineAuthority {
    memberships: AtomicUsize,
}

#[async_trait]
impl OrganizationAuthority for EditDeadlineAuthority {
    async fn organization_context(
        &self,
        caller: &crate::Caller,
        credential: &str,
        organization: &crate::SubjectId,
    ) -> Result<CallerContext, MembershipError> {
        assert_eq!(credential, "selected-credential");
        let lifetime = match self.memberships.fetch_add(1, Ordering::SeqCst) {
            0 | 2 => MAX_MEMBERSHIP_AGE,
            1 => Duration::from_secs(4),
            _ => panic!("unexpected membership refresh"),
        };
        let mut context = context(caller.subject.as_str());
        let now = SystemTime::now();
        context.memberships.push(OrganizationMembership::new(
            context.credential.clone(),
            organization.clone(),
            OrganizationRole::Admin,
            now,
            now + lifetime,
        )?);
        Ok(context)
    }

    async fn organization_choices(
        &self,
        _: &crate::Caller,
        _: &str,
    ) -> Result<Vec<OrganizationChoice>, MembershipError> {
        unreachable!()
    }
}

#[async_trait]
impl AgentCommandAdmissionAuthority for EditDeadlineAuthority {
    async fn command_admission_authority(
        &self,
        _: &CallerContext,
        _: &str,
        _: &AgentOwner,
    ) -> Result<AgentAuthorizationRevision, CommandAuthorizationError> {
        Ok(AgentAuthorizationRevision {
            replay_principal: None,
            authority_id: "admin-edit-custody".into(),
            policy_revision: 1,
            grant_revision: None,
        })
    }
}

#[tokio::test(start_paused = true)]
async fn queued_edit_keeps_the_tighter_deadline_across_a_later_refresh() {
    let (store, _, edit) = queued_edit_fixture().await;
    super::super::execution::grant(&store, ShareMode::OrgRoles).await;
    let authority = Arc::new(EditDeadlineAuthority {
        memberships: AtomicUsize::new(0),
    });
    let edits = Arc::new(EditStore {
        inner: store.clone(),
        requests: Mutex::default(),
        first_reply: FirstEditReply::StaleTwiceThenDelayed,
    });
    let service = AgentCommandService::new(
        edits.clone(),
        Arc::new(ResourceAccessService::new(store.clone(), authority.clone())),
        authority,
    );
    let started = tokio::time::Instant::now();
    assert!(matches!(
        service
            .edit_queued_input(
                &context("admin"),
                "selected-credential",
                CommandSubmissionSource::Api,
                edit.clone(),
            )
            .await,
        Err(QueuedCancellationError::Admission(
            CommandAdmissionError::Ledger(CommandLedgerError::Unavailable)
        ))
    ));
    let request = {
        let requests = edits.requests.lock().unwrap();
        assert_eq!(requests.len(), 3);
        let bound = started + Duration::from_secs(4);
        assert_eq!(
            requests[1].cancellation.command.authority_deadline,
            Some(bound)
        );
        assert_eq!(
            requests[2].cancellation.command.authority_deadline,
            Some(bound)
        );
        requests[2].cancellation.command.clone()
    };
    assert_eq!(started.elapsed(), Duration::from_secs(4));
    assert!(store
        .get_admitted_command(&request.scope(), &request.key_sha256)
        .await
        .unwrap()
        .is_none());
    let source = store
        .get_command(&edit.target.source_command_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(source.revision, 1);
    assert!(source.queued_input.is_none());
}

#[tokio::test(start_paused = true)]
async fn queued_edit_leaves_generic_conflicts_and_ambiguous_replies_for_explicit_recovery() {
    for first_reply in [
        FirstEditReply::Conflict,
        FirstEditReply::Lost,
        FirstEditReply::Delayed,
    ] {
        let first_conflicts = matches!(first_reply, FirstEditReply::Conflict);
        let delayed = matches!(first_reply, FirstEditReply::Delayed);
        let (store, authority, edit) = queued_edit_fixture().await;
        let edits = Arc::new(EditStore {
            inner: store.clone(),
            requests: Mutex::default(),
            first_reply,
        });
        let (_, slow_authority) =
            service(&store, authority.clone(), Progress::SlowFirstPreparation);
        let command_authority: Arc<dyn AgentCommandAdmissionAuthority> = if delayed {
            slow_authority
        } else {
            authority.clone()
        };
        let service = AgentCommandService::new(
            edits.clone(),
            Arc::new(ResourceAccessService::new(store.clone(), authority)),
            command_authority,
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
        let started = tokio::time::Instant::now();
        assert!(matches!(
            submit().await,
            Err(QueuedCancellationError::Admission(CommandAdmissionError::Ledger(error)))
                if error == expected
        ));
        if delayed {
            assert!((Duration::from_secs(15)..Duration::from_secs(16)).contains(&started.elapsed()));
        }
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
