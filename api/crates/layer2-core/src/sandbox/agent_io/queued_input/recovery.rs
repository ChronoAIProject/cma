//! Recover only a committed exact edit; never acquire ownership or create work.
use super::super::{
    AgentCommand, AgentCommandService, CommandAdmissionError, CommandLedgerError,
    CommandSubmission, CommandSubmissionSource, QueuedCancellationReservation,
};
use super::{QueuedInputReservation, QueuedInputStatusView, QueuedInputSubmission};
use crate::resource_access::{CallerContext, ResourceCapability, ResourceTarget};

impl AgentCommandService {
    pub async fn lookup_queued_input(
        &self,
        context: &CallerContext,
        credential: &str,
        source: CommandSubmissionSource,
        submission: QueuedInputSubmission,
    ) -> Result<Option<QueuedInputStatusView>, CommandAdmissionError> {
        super::queued_input_fingerprint(
            &submission.agent_id,
            &submission.target,
            &submission.text,
        )?;
        let (authorized, command) = self
            .prepare(
                context,
                credential,
                source,
                CommandSubmission {
                    agent_id: submission.agent_id,
                    command: AgentCommand::Interrupt,
                    idempotency_key: submission.idempotency_key,
                    expected_agent_revision: None,
                    requested_model: None,
                },
            )
            .await?;
        let agent = authorized
            .snapshot
            .lineage
            .agent
            .as_ref()
            .ok_or(CommandLedgerError::Corrupt)?;
        let resource = ResourceTarget::Agent {
            agent_id: agent.id.clone(),
        };
        let request = QueuedInputReservation {
            text: submission.text,
            cancellation: QueuedCancellationReservation {
                command,
                target: submission.target,
                observed_agent_revision: agent.revision,
                agent_created_at: agent.created_at.clone(),
            },
        };
        request.validate()?;
        let capability = ResourceCapability::OperateAgent;
        let result = self
            .with_authority_bound(
                &authorized,
                capability,
                self.store.lookup_queued_input(request.clone()),
            )
            .await;
        self.fence(&authorized, &resource, capability).await?;
        result??
            .map(|value| {
                request
                    .replay(value.record, Some(value.binding), value.response)
                    .map(Into::into)
                    .map_err(Into::into)
            })
            .transpose()
    }
}
