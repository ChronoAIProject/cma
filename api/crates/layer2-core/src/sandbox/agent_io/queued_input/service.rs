use super::super::{
    AgentCommand, AgentCommandService, CommandAdmissionError, CommandLedgerError,
    CommandSubmission, CommandSubmissionSource, QueuedCancellationReservation,
};
use super::{
    QueuedInputOutcome, QueuedInputReservation, QueuedInputStatusView, QueuedInputSubmission,
    QueuedInputView,
};
use crate::resource_access::{AccessPurpose, CallerContext, ResourceCapability, ResourceTarget};
impl AgentCommandService {
    pub async fn edit_queued_input(
        &self,
        context: &CallerContext,
        credential: &str,
        source: CommandSubmissionSource,
        submission: QueuedInputSubmission,
    ) -> Result<QueuedInputStatusView, super::super::QueuedCancellationError> {
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
                self.store.edit_queued_input(request.clone()),
            )
            .await;
        self.fence(&authorized, &resource, capability).await?;
        match result?? {
            QueuedInputOutcome::NotAdmitted => {
                Err(super::super::QueuedCancellationError::NotAdmitted)
            }
            QueuedInputOutcome::Applied(value) => {
                let value = request.replay(value.record, Some(value.binding), value.response)?;
                Ok(value.into())
            }
        }
    }
}

impl AgentCommandService {
    pub async fn list_queued_inputs(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        agent_id: &crate::AgentId,
    ) -> Result<Vec<QueuedInputView>, CommandAdmissionError> {
        if !context.validate_principal() || context.actor.ordinary_subject().is_none() {
            return Err(CommandLedgerError::NotFound.into());
        }
        let target = ResourceTarget::Agent {
            agent_id: agent_id.clone(),
        };
        let capability = ResourceCapability::OperateAgent;
        let authorized = super::super::admission_service::bounded(self.access.authorize(
            context,
            credential,
            &target,
            capability,
            AccessPurpose::Read,
        ))
        .await??;
        let result = self
            .with_authority_bound(
                &authorized,
                capability,
                self.store.list_queued_inputs(agent_id),
            )
            .await;
        self.fence(&authorized, &target, capability).await?;
        let agent = authorized
            .snapshot
            .lineage
            .agent
            .as_ref()
            .ok_or(CommandLedgerError::Corrupt)?;
        let owner = authorized
            .snapshot
            .scope(&target)
            .map_err(crate::resource_access::ResourceAccessError::from)?
            .owner;
        let records = result??;
        let mut values = Vec::new();
        for record in records {
            let response_id = record
                .receipt
                .response_id
                .as_ref()
                .ok_or(CommandLedgerError::Corrupt)?;
            super::super::queued_cancellation::reservation::validate_source(
                agent,
                &owner,
                &record,
                response_id,
            )?;
            values.push(QueuedInputView::from_record(&record)?);
        }
        Ok(values)
    }
}
