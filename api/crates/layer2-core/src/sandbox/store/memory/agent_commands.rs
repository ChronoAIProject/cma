use async_trait::async_trait;

use crate::ids::AgentCommandId;
use crate::sandbox::agent_io::{
    validate_command_reservation, AdmittedAgentCommand, AgentCommandStore, AgentResponseStatus,
    CommandLedgerError, CommandReservation, CommandReservationResult,
};
use crate::sandbox::store::InMemorySandboxStore;

#[async_trait]
impl AgentCommandStore for InMemorySandboxStore {
    async fn reserve_interrupting_message(
        &self,
        request: crate::sandbox::agent_io::InterruptingMessageReservation,
    ) -> Result<crate::sandbox::agent_io::AdmittedInterruptingMessage, CommandLedgerError> {
        super::interrupting_messages::reserve(self, request)
    }

    async fn edit_queued_input(
        &self,
        request: crate::sandbox::agent_io::QueuedInputReservation,
    ) -> Result<crate::sandbox::agent_io::QueuedInputOutcome, CommandLedgerError> {
        super::queued_inputs::apply(self, request)
    }
    async fn list_queued_inputs(
        &self,
        agent_id: &crate::AgentId,
    ) -> Result<Vec<AdmittedAgentCommand>, CommandLedgerError> {
        let state = self.lock();
        let mut records: Vec<_> = state
            .agent_commands
            .values()
            .filter(|record| {
                &record.receipt.envelope.agent_id == agent_id
                    && crate::sandbox::agent_io::queued_cancellation_eligible(record)
                    && crate::sandbox::agent_io::queued_input_editable_actor(record)
            })
            .cloned()
            .collect();
        records.sort_by_key(|record| record.receipt.agent_order);
        if records.len() > crate::sandbox::agent_io::MAX_COMMAND_QUEUE_CAPACITY as usize {
            return Err(CommandLedgerError::Corrupt);
        }
        for record in &records {
            record.validate()?;
        }
        Ok(records)
    }

    async fn cancel_queued_response(
        &self,
        request: crate::sandbox::agent_io::QueuedCancellationReservation,
    ) -> Result<crate::sandbox::agent_io::QueuedCancellationOutcome, CommandLedgerError> {
        super::queued_cancellations::apply(self, request)
    }

    async fn reserve_targeted_control(
        &self,
        request: crate::sandbox::agent_io::TargetedControlReservation,
    ) -> Result<crate::sandbox::agent_io::AdmittedTargetedControl, CommandLedgerError> {
        super::targeted_controls::apply(self, request, true)?.ok_or(CommandLedgerError::Corrupt)
    }

    async fn lookup_targeted_control(
        &self,
        request: crate::sandbox::agent_io::TargetedControlReservation,
    ) -> Result<Option<crate::sandbox::agent_io::AdmittedTargetedControl>, CommandLedgerError> {
        super::targeted_controls::apply(self, request, false)
    }

    async fn inspect_targeted_control(
        &self,
        request: crate::sandbox::agent_io::TargetedControlReservation,
    ) -> Result<crate::sandbox::agent_io::TargetedControlInspection, CommandLedgerError> {
        super::targeted_controls::inspect(self, request)
    }

    async fn get_admitted_command(
        &self,
        scope: &str,
        key_sha256: &str,
    ) -> Result<Option<AdmittedAgentCommand>, CommandLedgerError> {
        let state = self.lock();
        let Some(id) = state
            .agent_command_keys
            .get(&(scope.into(), key_sha256.into()))
        else {
            return Ok(None);
        };
        let record = state
            .agent_commands
            .get(id)
            .ok_or(CommandLedgerError::Corrupt)?;
        record.validate()?;
        if record.scope() != scope || record.key_sha256 != key_sha256 {
            return Err(CommandLedgerError::Corrupt);
        }
        Ok(Some(record.clone()))
    }

    async fn get_external_event_command(
        &self,
        key: &str,
    ) -> Result<Option<AdmittedAgentCommand>, CommandLedgerError> {
        let state = self.lock();
        let mut matches = state
            .agent_commands
            .values()
            .filter(|record| record.receipt.envelope.external_event_key().as_deref() == Some(key));
        let record = matches.next().cloned();
        if matches.next().is_some() {
            return Err(CommandLedgerError::Corrupt);
        }
        if let Some(record) = &record {
            record.validate()?;
        }
        Ok(record)
    }

    async fn get_operation_command(
        &self,
        agent_id: &crate::AgentId,
        operation_sequence: u64,
    ) -> Result<Option<AdmittedAgentCommand>, CommandLedgerError> {
        let state = self.lock();
        let mut matches = state.agent_commands.values().filter(|record| {
            &record.receipt.envelope.agent_id == agent_id
                && record
                    .work
                    .dispatch
                    .as_ref()
                    .is_some_and(|dispatch| dispatch.operation_sequence == operation_sequence)
        });
        let record = matches.next().cloned();
        if matches.next().is_some() {
            return Err(CommandLedgerError::Corrupt);
        }
        if let Some(record) = &record {
            record.validate()?;
        }
        Ok(record)
    }

    async fn get_response_command(
        &self,
        id: &crate::ids::AgentResponseId,
    ) -> Result<Option<AdmittedAgentCommand>, CommandLedgerError> {
        let state = self.lock();
        let mut matches = state
            .agent_commands
            .values()
            .filter(|command| command.receipt.response_id.as_ref() == Some(id));
        let record = matches.next().cloned();
        if matches.next().is_some() {
            return Err(CommandLedgerError::Corrupt);
        }
        if let Some(record) = &record {
            record.validate()?;
        }
        Ok(record)
    }

    async fn reserve_command(
        &self,
        request: CommandReservation,
    ) -> Result<CommandReservationResult, CommandLedgerError> {
        request.require_non_channel()?;
        request.validate()?;
        let mut state = self.lock();
        reserve(&mut state, &request)
    }

    async fn get_command(
        &self,
        id: &AgentCommandId,
    ) -> Result<Option<AdmittedAgentCommand>, CommandLedgerError> {
        let record = self.lock().agent_commands.get(id).cloned();
        if let Some(record) = &record {
            record.validate()?;
        }
        Ok(record)
    }
}

/// Shared locked insertion; targeted admission stages a candidate before publishing.
pub(super) fn reserve(
    state: &mut super::super::State,
    request: &CommandReservation,
) -> Result<CommandReservationResult, CommandLedgerError> {
    request.require_current_authority()?;
    super::bindings::require_conversation_authority(state, request.binding.as_ref())
        .map_err(|_| CommandLedgerError::Conflict)?;
    let key = (request.scope(), request.key_sha256.clone());
    if let Some(id) = state.agent_command_keys.get(&key) {
        let current = state
            .agent_commands
            .get(id)
            .ok_or(CommandLedgerError::Corrupt)?;
        return request.replay(current.clone());
    }
    if let Some(event) = request.envelope.external_event_key() {
        if state
            .agent_commands
            .values()
            .any(|record| record.receipt.envelope.external_event_key().as_ref() == Some(&event))
        {
            return Err(CommandLedgerError::IdempotencyConflict);
        }
    }
    let agent = state
        .agents
        .get(&request.envelope.agent_id)
        .ok_or(CommandLedgerError::NotFound)?;
    let parent = state
        .sandboxes
        .get(&agent.sandbox_id)
        .ok_or(CommandLedgerError::NotFound)?;
    validate_command_reservation(request, agent, parent)?;
    if agent.bootstrap_operation(parent).is_some()
        && state
            .workspaces
            .get(&agent.workspace_id)
            .is_none_or(|workspace| agent.bootstrap_lineage(parent, workspace).is_none())
    {
        return Err(CommandLedgerError::NotAdmissible);
    }
    crate::sandbox::agent_io::validate_command_reservation_policy(
        request,
        parent,
        state.resource_policies.get(&parent.id),
    )?;
    let waiting = state
        .agent_commands
        .values()
        .filter(|command| {
            command.receipt.envelope.agent_id == agent.id
                && command.status == AgentResponseStatus::Queued
        })
        .count();
    if waiting >= usize::from(request.queue_capacity) {
        return Err(CommandLedgerError::QueueFull);
    }
    if state
        .agent_commands
        .contains_key(&request.envelope.command_id)
        || request.response_id.as_ref().is_some_and(|id| {
            state
                .agent_commands
                .values()
                .any(|command| command.receipt.response_id.as_ref() == Some(id))
        })
    {
        return Err(CommandLedgerError::Conflict);
    }
    let order = state
        .agent_command_orders
        .get(&agent.id)
        .map_or(0, |lane| lane.last_order)
        .checked_add(1)
        .ok_or(CommandLedgerError::Corrupt)?;
    let target_turn = (!matches!(
        request.envelope.command,
        crate::sandbox::agent_io::AgentCommand::Message { .. }
    ))
    .then_some(agent.conversation.active_turn)
    .flatten();
    let record = request.record(order, agent.conversation.last_seq, target_turn)?;
    let id = record.receipt.envelope.command_id.clone();
    if let Some(evidence) = &request.admission_audit {
        let audit = crate::sandbox::agent_io::CommandAuthorizationAudit::new(
            &record,
            crate::sandbox::agent_io::CommandAuthorizationEffect::Admission,
            vec![evidence.clone()],
        )?;
        super::command_audit::append(state, audit)?;
    }
    state
        .agent_command_orders
        .entry(request.envelope.agent_id.clone())
        .or_default()
        .last_order = order;
    state.agent_command_keys.insert(key, id.clone());
    state.agent_commands.insert(id, record.clone());
    Ok(CommandReservationResult::Created(Box::new(record)))
}
