use super::*;
use crate::ids::AgentCommandId;

struct BudgetObservation {
    inner: Arc<InMemorySandboxStore>,
    inspected: Mutex<Option<TargetedControlReservation>>,
}

#[async_trait]
impl AgentCommandStore for BudgetObservation {
    async fn inspect_targeted_control(
        &self,
        request: TargetedControlReservation,
    ) -> Result<TargetedControlInspection, CommandLedgerError> {
        let deadline = request.retry_deadline.expect("public request has a budget");
        assert_eq!(
            deadline - tokio::time::Instant::now(),
            Duration::from_secs(5)
        );
        let result = self.inner.inspect_targeted_control(request.clone()).await?;
        *self.inspected.lock().unwrap() = Some(request);
        // Spend four seconds in the inspection phase. The reservation phase
        // must inherit one remaining second, not receive a fresh five seconds.
        tokio::time::advance(Duration::from_secs(4)).await;
        Ok(result)
    }

    async fn reserve_targeted_control(
        &self,
        request: TargetedControlReservation,
    ) -> Result<AdmittedTargetedControl, CommandLedgerError> {
        let inspected = self.inspected.lock().unwrap().take().unwrap();
        assert_eq!(request.retry_deadline, inspected.retry_deadline);
        assert_eq!(
            request.retry_deadline.unwrap() - tokio::time::Instant::now(),
            Duration::from_secs(1)
        );
        assert_eq!(
            request.command.authority_deadline,
            inspected.command.authority_deadline
        );
        assert_eq!(request.fingerprint(), inspected.fingerprint());
        assert_eq!(request.command.key_sha256, inspected.command.key_sha256);
        assert_eq!(request.target, inspected.target);
        self.inner.reserve_targeted_control(request).await
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
        id: &crate::ids::AgentResponseId,
    ) -> Result<Option<AdmittedAgentCommand>, CommandLedgerError> {
        self.inner.get_response_command(id).await
    }
}

#[tokio::test(start_paused = true)]
async fn public_control_shares_retry_budget_across_inspection_and_reservation() {
    let (store, _, authority, submission) = fixture().await;
    let access = Arc::new(ResourceAccessService::new(store.clone(), authority.clone()));
    let observed = Arc::new(BudgetObservation {
        inner: store,
        inspected: Mutex::new(None),
    });
    let service = AgentCommandService::new(observed.clone(), access, authority);
    let accepted = service
        .admit_response_control(
            &context("owner"),
            "selected-credential",
            CommandSubmissionSource::Api,
            submission.clone(),
        )
        .await
        .unwrap();
    assert_eq!(accepted.target, submission.target);
    assert!(observed.inspected.lock().unwrap().is_none());
}
