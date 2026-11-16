use crate::resource_access::{CallerContext, ResourceCapability, ResourceTarget};

use super::super::{
    AgentCommand, AgentCommandService, CommandAdmissionError, CommandLedgerError,
    CommandSubmission, CommandSubmissionSource, OwnerCommandMutation, OwnerCommandReply,
};
use super::{
    QueuedCancellationOutcome, QueuedCancellationReservation, QueuedCancellationStatusView,
    QueuedCancellationSubmission,
};

#[derive(Debug, thiserror::Error)]
pub enum QueuedCancellationError {
    #[error(transparent)]
    Admission(#[from] CommandAdmissionError),
    #[error("the original queued cancellation was not admitted and its source revision is permanently stale")]
    NotAdmitted,
}

impl From<CommandLedgerError> for QueuedCancellationError {
    fn from(error: CommandLedgerError) -> Self {
        Self::Admission(error.into())
    }
}

impl AgentCommandService {
    pub async fn cancel_queued_response(
        &self,
        context: &CallerContext,
        credential: &str,
        source: CommandSubmissionSource,
        submission: QueuedCancellationSubmission,
    ) -> Result<QueuedCancellationStatusView, QueuedCancellationError> {
        submission.target.fingerprint(&submission.agent_id)?;
        match self
            .route_mutation(
                context,
                credential,
                source,
                OwnerCommandMutation::QueuedCancellation(submission.clone()),
            )
            .await?
        {
            Some(OwnerCommandReply::QueuedCancellation(result)) => return result,
            Some(_) => return Err(CommandLedgerError::Unavailable.into()),
            None => {}
        }
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
        let request = QueuedCancellationReservation {
            command,
            target: submission.target,
            observed_agent_revision: agent.revision,
            agent_created_at: agent.created_at.clone(),
        };
        request.validate()?;
        let capability = ResourceCapability::OperateAgent;
        let result = self
            .with_authority_bound(
                &authorized,
                capability,
                self.store.cancel_queued_response(request.clone()),
            )
            .await;
        self.fence(&authorized, &resource, capability).await?;
        match result?? {
            QueuedCancellationOutcome::NotAdmitted => Err(QueuedCancellationError::NotAdmitted),
            QueuedCancellationOutcome::Applied(value) => {
                let value = request.replay(value.record, Some(value.binding), value.response)?;
                Ok(value.into())
            }
        }
    }
}
