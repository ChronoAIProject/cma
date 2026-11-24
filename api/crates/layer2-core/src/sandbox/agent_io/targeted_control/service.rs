use crate::resource_access::{AuthorizedResource, CallerContext, ResourceTarget};

use super::super::admission_service::command_capability;
use super::super::{
    AgentCommandService, CommandAdmissionError, CommandLedgerError, CommandSubmission,
    CommandSubmissionSource,
};
use super::{TargetedControlReservation, TargetedControlStatusView, TargetedControlSubmission};

impl AgentCommandService {
    pub async fn admit_targeted_control(
        &self,
        context: &CallerContext,
        credential: &str,
        source: CommandSubmissionSource,
        submission: TargetedControlSubmission,
    ) -> Result<TargetedControlStatusView, CommandAdmissionError> {
        let (authorized, request) = self
            .prepare_targeted(context, credential, source, submission)
            .await?;
        let target = ResourceTarget::Agent {
            agent_id: request.command.envelope.agent_id.clone(),
        };
        let capability = command_capability(&request.command.envelope.command);
        let result = self
            .with_authority_bound(
                &authorized,
                capability,
                self.store.reserve_targeted_control(request.clone()),
            )
            .await;
        self.fence(&authorized, &target, capability).await?;
        let result = result??;
        let result = request.replay(result.record, Some(result.binding))?;
        self.notify_new_cancel(&request.command.envelope, &result.record)
            .await;
        Ok(TargetedControlStatusView::from_record(&result))
    }

    pub async fn lookup_targeted_control(
        &self,
        context: &CallerContext,
        credential: &str,
        source: CommandSubmissionSource,
        submission: TargetedControlSubmission,
    ) -> Result<Option<TargetedControlStatusView>, CommandAdmissionError> {
        let (authorized, request) = self
            .prepare_targeted(context, credential, source, submission)
            .await?;
        let target = ResourceTarget::Agent {
            agent_id: request.command.envelope.agent_id.clone(),
        };
        let capability = command_capability(&request.command.envelope.command);
        let result = self
            .with_authority_bound(
                &authorized,
                capability,
                self.store.lookup_targeted_control(request.clone()),
            )
            .await;
        self.fence(&authorized, &target, capability).await?;
        let Some(result) = result?? else {
            return Ok(None);
        };
        let result = request.replay(result.record, Some(result.binding))?;
        Ok(Some(TargetedControlStatusView::from_record(&result)))
    }

    pub(in crate::sandbox::agent_io) async fn prepare_targeted(
        &self,
        context: &CallerContext,
        credential: &str,
        source: CommandSubmissionSource,
        submission: TargetedControlSubmission,
    ) -> Result<(AuthorizedResource, TargetedControlReservation), CommandAdmissionError> {
        submission.target.fingerprint(
            &submission.agent_id,
            submission.expected_agent_revision,
            &submission.command,
        )?;
        let (authorized, command) = self
            .prepare(
                context,
                credential,
                source,
                CommandSubmission {
                    agent_id: submission.agent_id,
                    command: submission.command,
                    idempotency_key: submission.idempotency_key,
                    expected_agent_revision: Some(submission.expected_agent_revision),
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
        let request = TargetedControlReservation {
            command,
            target: submission.target,
            observed_agent_revision: agent.revision,
            agent_created_at: agent.created_at.clone(),
            retry_deadline: None,
        };
        request.validate()?;
        Ok((authorized, request))
    }
}
