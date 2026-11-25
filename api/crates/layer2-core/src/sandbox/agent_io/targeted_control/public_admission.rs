use crate::resource_access::{CallerContext, ResourceTarget};

use super::super::admission_service::command_capability;
use super::super::{
    AgentCommandService, CommandAdmissionError, CommandLedgerError, CommandSubmissionSource,
};
use super::{
    ResponseControlRuntimeSupport, TargetedControlInspection, TargetedControlReservation,
    TargetedControlStatusView, TargetedControlSubmission,
};

/// Additive interface error; v1 admission errors and persisted failures retain
/// their existing meanings. A later capability race remains a generic conflict.
#[derive(Debug, thiserror::Error)]
pub enum TargetedControlError {
    #[error(transparent)]
    Admission(#[from] CommandAdmissionError),
    #[error("the observed runtime does not support this control operation")]
    CapabilityUnsupported,
    #[error("the original response control intent was not admitted and is permanently stale")]
    NotAdmitted,
}

impl From<CommandLedgerError> for TargetedControlError {
    fn from(error: CommandLedgerError) -> Self {
        Self::Admission(error.into())
    }
}

impl AgentCommandService {
    /// Exact receipt recovery precedes target/capability checks. All reads and
    /// writes retain the same fresh operation authorization and bounded fence.
    pub async fn admit_response_control(
        &self,
        context: &CallerContext,
        credential: &str,
        source: CommandSubmissionSource,
        submission: TargetedControlSubmission,
    ) -> Result<TargetedControlStatusView, TargetedControlError> {
        let (authorized, mut request) = self
            .prepare_targeted(context, credential, source, submission)
            .await?;
        // Both store phases share one retry scheduling budget. Active database
        // attempts and ambiguous commits must still finish normally; this is
        // separate from the original credential authority deadline.
        request.retry_deadline =
            Some(tokio::time::Instant::now() + std::time::Duration::from_secs(5));
        let resource = ResourceTarget::Agent {
            agent_id: request.command.envelope.agent_id.clone(),
        };
        let capability = command_capability(&request.command.envelope.command);
        let result = self
            .with_authority_bound(&authorized, capability, async {
                match self
                    .store
                    .inspect_targeted_control(request.clone())
                    .await
                    .map_err(|error| trace_rejection(&request, "inspect", error))?
                {
                    TargetedControlInspection::Existing(existing) => return Ok(*existing),
                    TargetedControlInspection::NotAdmitted => {
                        return Err(TargetedControlError::NotAdmitted)
                    }
                    TargetedControlInspection::Missing => {}
                }
                let agent = authorized
                    .snapshot
                    .lineage
                    .agent
                    .as_ref()
                    .ok_or(CommandLedgerError::Corrupt)?;
                let response = self
                    .store
                    .get_response_command(&request.target.response_id)
                    .await?
                    .ok_or(CommandLedgerError::NotFound)?;
                // A stale target cannot be mislabeled as an unsupported operation.
                request
                    .validate_target(agent, &response)
                    .map_err(|error| trace_rejection(&request, "observed_target", error))?;
                if !ResponseControlRuntimeSupport::from_runtime(&agent.capabilities)
                    .supports(&request.command.envelope.command)
                {
                    return Err(TargetedControlError::CapabilityUnsupported);
                }
                self.require_model_credit(&request.command.envelope.command)
                    .map_err(CommandAdmissionError::from)?;
                Ok(self
                    .store
                    .reserve_targeted_control(request.clone())
                    .await
                    .map_err(|error| trace_rejection(&request, "reserve", error))?)
            })
            .await;
        self.fence(&authorized, &resource, capability).await?;
        let result = result??;
        let result = request.replay(result.record, Some(result.binding))?;
        self.notify_new_cancel(&request.command.envelope, &result.record)
            .await;
        Ok(TargetedControlStatusView::from_record(&result))
    }
}

fn trace_rejection(
    request: &TargetedControlReservation,
    stage: &'static str,
    error: CommandLedgerError,
) -> CommandLedgerError {
    tracing::warn!(
        agent_id = %request.command.envelope.agent_id,
        response_id = %request.target.response_id,
        expected_agent_revision = request.command.expected_agent_revision,
        observed_agent_revision = request.observed_agent_revision,
        stage,
        code = error.code(),
        "response control public admission refused"
    );
    error
}
