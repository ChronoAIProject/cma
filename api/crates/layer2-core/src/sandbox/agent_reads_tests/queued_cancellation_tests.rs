use super::*;
use crate::ids::{AgentCommandId, AgentResponseId};

struct ChangePolicyAfterCommit(Arc<InMemorySandboxStore>);
#[async_trait]
impl AgentCommandStore for ChangePolicyAfterCommit {
    async fn cancel_queued_response(
        &self,
        request: QueuedCancellationReservation,
    ) -> Result<QueuedCancellationOutcome, CommandLedgerError> {
        let result = self.0.cancel_queued_response(request).await?;
        share(&self.0, None).await;
        Ok(result)
    }
    async fn reserve_command(
        &self,
        request: CommandReservation,
    ) -> Result<CommandReservationResult, CommandLedgerError> {
        self.0.reserve_command(request).await
    }
    async fn get_command(
        &self,
        id: &AgentCommandId,
    ) -> Result<Option<AdmittedAgentCommand>, CommandLedgerError> {
        self.0.get_command(id).await
    }
    async fn get_response_command(
        &self,
        id: &AgentResponseId,
    ) -> Result<Option<AdmittedAgentCommand>, CommandLedgerError> {
        self.0.get_response_command(id).await
    }
}

async fn queued() -> (
    Arc<InMemorySandboxStore>,
    Arc<AgentCommandService>,
    Arc<Authority>,
    QueuedCancellationSubmission,
) {
    let (store, service, authority, agent) = setup(Authority::new()).await;
    let source = service
        .admit(
            &context("owner"),
            "selected-credential",
            CommandSubmissionSource::Api,
            submission("queued-source"),
        )
        .await
        .unwrap();
    let request = QueuedCancellationSubmission {
        agent_id: agent.id,
        target: QueuedCancellationTarget {
            response_id: source.receipt.response_id.unwrap(),
            source_command_id: source.receipt.command_id,
            expected_command_revision: 1,
        },
        idempotency_key: "queued-cancellation".into(),
    };
    (store, service, authority, request)
}

#[tokio::test]
async fn queued_cancellation_post_commit_policy_change_hides_effect_and_refusal_until_fresh_recovery(
) {
    for stale in [false, true] {
        let (store, service, authority, request) = queued().await;
        if stale {
            store
                .reject_unclaimed_command(
                    &request.target.source_command_id,
                    1,
                    CommandFailureCode::AgentUnavailable,
                    crate::now_rfc3339(),
                )
                .await
                .unwrap();
        }
        let access = Arc::new(ResourceAccessService::new(store.clone(), authority.clone()));
        let gated = AgentCommandService::new(
            Arc::new(ChangePolicyAfterCommit(store.clone())),
            access,
            authority,
        );
        assert!(matches!(
            gated
                .cancel_queued_response(
                    &context("owner"),
                    "selected-credential",
                    CommandSubmissionSource::Api,
                    request.clone()
                )
                .await,
            Err(QueuedCancellationError::Admission(
                CommandAdmissionError::Access(_)
            ))
        ));
        let recovered = service
            .cancel_queued_response(
                &context("owner"),
                "selected-credential",
                CommandSubmissionSource::Api,
                request.clone(),
            )
            .await;
        if stale {
            assert!(matches!(
                recovered,
                Err(QueuedCancellationError::NotAdmitted)
            ));
        } else {
            let accepted = recovered.unwrap();
            assert_eq!(accepted.cancellation.status, AgentResponseStatus::Completed);
            assert_eq!(accepted.response.status, AgentResponseStatus::Cancelled);
            assert_eq!(accepted.target, request.target);
            assert_eq!(
                store
                    .command_authorization_audits(
                        &accepted.cancellation.receipt.command_id,
                        &PageRequest::default()
                    )
                    .await
                    .unwrap()
                    .items
                    .len(),
                1
            );
        }
    }
}

#[tokio::test]
async fn queued_cancellation_history_view_and_forged_identity_do_not_authorize_observation_or_mutation(
) {
    let (store, service, authority, request) = queued().await;
    share(&store, None).await;
    let calls = authority.calls.load(Ordering::SeqCst);
    let mut forged = context("owner");
    forged.actor = context("reader").actor;
    let mut delegated = context("owner");
    delegated.actor = AgentActor::Api {
        subject: SubjectId::new("owner"),
        delegation_id: Some("external".into()),
    };
    for caller in [context("reader"), context("unrelated"), forged, delegated] {
        assert!(service
            .observe_queued_cancellation(
                &caller,
                Some("selected-credential"),
                &request.agent_id,
                &request.target.response_id
            )
            .await
            .is_err());
        assert!(service
            .cancel_queued_response(
                &caller,
                "selected-credential",
                CommandSubmissionSource::Api,
                request.clone()
            )
            .await
            .is_err());
    }
    assert_eq!(authority.calls.load(Ordering::SeqCst), calls);
    assert_eq!(
        store
            .get_command(&request.target.source_command_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        AgentResponseStatus::Queued
    );
}
