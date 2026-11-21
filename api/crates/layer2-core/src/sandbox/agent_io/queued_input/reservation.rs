use super::super::{
    AdmittedAgentCommand, AgentResponseStatus, CommandLane, CommandLedgerError, CommandWorkState,
    QueuedCancellationReservation,
};
use super::{
    queued_input_editable_actor, queued_input_fingerprint, AdmittedQueuedInput, QueuedInputBinding,
    QueuedInputRevision,
};
#[derive(Debug, Clone)]
pub struct QueuedInputReservation {
    pub cancellation: QueuedCancellationReservation,
    pub text: String,
}
impl QueuedInputReservation {
    pub fn validate(&self) -> Result<(), CommandLedgerError> {
        self.cancellation.validate()?;
        self.fingerprint()?;
        Ok(())
    }
    pub fn fingerprint(&self) -> Result<String, CommandLedgerError> {
        queued_input_fingerprint(
            &self.cancellation.command.envelope.agent_id,
            &self.cancellation.target,
            &self.text,
        )
    }
    pub fn revise(
        &self,
        response: &mut AdmittedAgentCommand,
        lane: &mut CommandLane,
    ) -> Result<(), CommandLedgerError> {
        if !queued_input_editable_actor(response) {
            return Err(CommandLedgerError::NotAdmissible);
        }
        // Reuse cancellation's atomic eligibility, lease invalidation and revision fence.
        self.cancellation.cancel(response, lane)?;
        response.status = AgentResponseStatus::Queued;
        response.queued_input = Some(QueuedInputRevision {
            schema_version: 1,
            command_id: response.receipt.envelope.command_id.clone(),
            command_revision: response.revision,
            text: self.text.clone(),
        });
        response.validate()
    }
    pub fn bind(
        &self,
        record: AdmittedAgentCommand,
        response: AdmittedAgentCommand,
    ) -> Result<AdmittedQueuedInput, CommandLedgerError> {
        let binding = QueuedInputBinding {
            schema_version: 1,
            command_id: record.receipt.envelope.command_id.clone(),
            target: self.cancellation.target.clone(),
            text: self.text.clone(),
            command_revision: response.revision,
            agent_created_at: self.cancellation.agent_created_at.clone(),
            request_fingerprint: self.fingerprint()?,
        };
        self.replay(record, Some(binding), response)
    }
    pub fn replay(
        &self,
        record: AdmittedAgentCommand,
        binding: Option<QueuedInputBinding>,
        response: AdmittedAgentCommand,
    ) -> Result<AdmittedQueuedInput, CommandLedgerError> {
        self.cancellation.command.replay(record.clone())?;
        let binding = binding.ok_or(CommandLedgerError::IdempotencyConflict)?;
        response.validate()?;
        if binding.schema_version != 1
            || binding.command_id != record.receipt.envelope.command_id
            || binding.command_id == binding.target.source_command_id
            || binding.request_fingerprint
                != queued_input_fingerprint(
                    &record.receipt.envelope.agent_id,
                    &binding.target,
                    &binding.text,
                )?
            || binding.command_revision
                != binding
                    .target
                    .expected_command_revision
                    .checked_add(1)
                    .ok_or(CommandLedgerError::Corrupt)?
            || response.revision < binding.command_revision
            || record.status != AgentResponseStatus::Completed
            || record.work != CommandWorkState::default()
            || record.receipt.response_id.is_some()
            || record.target_turn.is_some()
            || response.receipt.envelope.command_id != binding.target.source_command_id
            || response.receipt.response_id.as_ref() != Some(&binding.target.response_id)
            || response.receipt.envelope.owner != record.receipt.envelope.owner
            || response.receipt.envelope.agent_id != record.receipt.envelope.agent_id
        {
            return Err(CommandLedgerError::Corrupt);
        }
        if binding.request_fingerprint != self.fingerprint()? {
            return Err(CommandLedgerError::IdempotencyConflict);
        }
        if binding.agent_created_at != self.cancellation.agent_created_at {
            return Err(CommandLedgerError::NotFound);
        }
        Ok(AdmittedQueuedInput {
            record,
            binding,
            response,
        })
    }
}
