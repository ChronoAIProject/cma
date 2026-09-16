use super::*;
use crate::ids::AgentCommandId;

struct RevokeAfterCommit(Arc<InMemorySandboxStore>);
#[async_trait]
impl AgentCommandStore for RevokeAfterCommit {
    async fn inspect_targeted_control(
        &self,
        request: TargetedControlReservation,
    ) -> Result<TargetedControlInspection, CommandLedgerError> {
        let result = self.0.inspect_targeted_control(request).await?;
        share(&self.0, None).await;
        Ok(result)
    }

    async fn reserve_targeted_control(
        &self,
        request: TargetedControlReservation,
    ) -> Result<AdmittedTargetedControl, CommandLedgerError> {
        let result = self.0.reserve_targeted_control(request).await?;
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
        id: &crate::ids::AgentResponseId,
    ) -> Result<Option<AdmittedAgentCommand>, CommandLedgerError> {
        self.0.get_response_command(id).await
    }
}

#[tokio::test]
async fn post_commit_authority_change_hides_reply_and_fresh_retry_recovers_acceptance() {
    let (store, service, authority, request) = fixture().await;
    let access = Arc::new(ResourceAccessService::new(store.clone(), authority.clone()));
    let gated = AgentCommandService::new(
        Arc::new(RevokeAfterCommit(store.clone())),
        access,
        authority,
    );
    assert!(matches!(
        gated
            .admit_targeted_control(
                &context("owner"),
                "selected-credential",
                CommandSubmissionSource::Api,
                request.clone()
            )
            .await,
        Err(CommandAdmissionError::Access(_))
    ));
    let recovered = service
        .lookup_targeted_control(
            &context("owner"),
            "selected-credential",
            CommandSubmissionSource::Api,
            request,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(recovered.control.status, AgentResponseStatus::Queued);
    assert_eq!(
        store
            .command_work_page(&PageRequest::default())
            .await
            .unwrap()
            .items
            .len(),
        2
    );
    assert_eq!(
        store
            .command_authorization_audits(
                &recovered.control.receipt.command_id,
                &PageRequest::default()
            )
            .await
            .unwrap()
            .items
            .len(),
        1
    );
}

#[tokio::test]
async fn post_inspection_authority_change_hides_refusal_until_fresh_authorized_recovery() {
    let (store, service, authority, request) = fixture().await;
    let mut agent = store.get_agent(&request.agent_id).await.unwrap().unwrap();
    let previous = agent.revision;
    agent.revision += 1;
    store.save_agent(previous, agent).await.unwrap();
    let access = Arc::new(ResourceAccessService::new(store.clone(), authority.clone()));
    let gated = AgentCommandService::new(
        Arc::new(RevokeAfterCommit(store.clone())),
        access,
        authority,
    );
    assert!(matches!(
        gated
            .admit_response_control(
                &context("owner"),
                "selected-credential",
                CommandSubmissionSource::Api,
                request.clone(),
            )
            .await,
        Err(TargetedControlError::Admission(
            CommandAdmissionError::Access(_)
        ))
    ));
    assert!(matches!(
        service
            .admit_response_control(
                &context("owner"),
                "selected-credential",
                CommandSubmissionSource::Api,
                request,
            )
            .await,
        Err(TargetedControlError::NotAdmitted)
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
