use crate::sandbox::agent_io::{
    validate_command_reservation_policy, AgentResponseStatus, CommandAuthorizationAudit,
    CommandAuthorizationEffect, CommandLedgerError, QueuedCancellationOutcome,
    QueuedCancellationReservation,
};
use crate::sandbox::store::InMemorySandboxStore;

pub(super) fn apply(
    store: &InMemorySandboxStore,
    request: QueuedCancellationReservation,
) -> Result<QueuedCancellationOutcome, CommandLedgerError> {
    request.validate()?;
    let mut state = store.lock();
    super::bindings::require_conversation_authority(&state, request.command.binding.as_ref())
        .map_err(|_| CommandLedgerError::Conflict)?;
    let key = (request.command.scope(), request.command.key_sha256.clone());
    let existing = state
        .agent_command_keys
        .get(&key)
        .map(|id| {
            let record = state
                .agent_commands
                .get(id)
                .ok_or(CommandLedgerError::Corrupt)?;
            request.command.replay(record.clone())?;
            let binding = state
                .queued_cancellations
                .get(id)
                .ok_or(CommandLedgerError::IdempotencyConflict)?;
            if binding.request_fingerprint
                != request
                    .target
                    .fingerprint(&request.command.envelope.agent_id)?
            {
                return Err(CommandLedgerError::IdempotencyConflict);
            }
            let response = state
                .agent_commands
                .get(&binding.target.source_command_id)
                .ok_or(CommandLedgerError::Corrupt)?;
            request.replay(record.clone(), Some(binding.clone()), response.clone())
        })
        .transpose()?;
    let agent = state
        .agents
        .get(&request.command.envelope.agent_id)
        .ok_or(CommandLedgerError::NotFound)?;
    let parent = state
        .sandboxes
        .get(&agent.sandbox_id)
        .ok_or(CommandLedgerError::NotFound)?;
    request.validate_current(agent, parent)?;
    validate_command_reservation_policy(
        &request.command,
        parent,
        state.resource_policies.get(&parent.id),
    )?;
    if let Some(existing) = existing {
        request.command.require_current_authority()?;
        return Ok(QueuedCancellationOutcome::Applied(Box::new(existing)));
    }
    let mut response = state
        .agent_commands
        .get(&request.target.source_command_id)
        .cloned()
        .ok_or(CommandLedgerError::NotFound)?;
    request.validate_source(agent, &response)?;
    if request.target.expected_command_revision < response.revision {
        request.command.require_current_authority()?;
        return Ok(QueuedCancellationOutcome::NotAdmitted);
    }
    let mut lane = state
        .agent_command_orders
        .get(&agent.id)
        .cloned()
        .ok_or(CommandLedgerError::Corrupt)?;
    if lane.last_order < response.receipt.agent_order {
        return Err(CommandLedgerError::Corrupt);
    }
    request.cancel(&mut response, &mut lane)?;
    lane.last_order = lane
        .last_order
        .checked_add(1)
        .filter(|order| *order <= i64::MAX as u64)
        .ok_or(CommandLedgerError::Corrupt)?;
    let mut record = request
        .command
        .record(lane.last_order, agent.conversation.last_seq, None)?;
    record.status = AgentResponseStatus::Completed;
    let result = request.bind(record, response)?;
    let id = result.record.receipt.envelope.command_id.clone();
    if state.agent_commands.contains_key(&id) {
        return Err(CommandLedgerError::Conflict);
    }
    let mut candidate = state.clone();
    if let Some(evidence) = &request.command.admission_audit {
        super::command_audit::append(
            &mut candidate,
            CommandAuthorizationAudit::new(
                &result.record,
                CommandAuthorizationEffect::Admission,
                vec![evidence.clone()],
            )?,
        )?;
    }
    candidate
        .agent_command_orders
        .insert(request.command.envelope.agent_id.clone(), lane);
    candidate.agent_commands.insert(
        result.binding.target.source_command_id.clone(),
        result.response.clone(),
    );
    candidate.agent_command_keys.insert(key, id.clone());
    candidate
        .agent_commands
        .insert(id.clone(), result.record.clone());
    if candidate
        .queued_cancellations
        .insert(id, result.binding.clone())
        .is_some()
    {
        return Err(CommandLedgerError::Corrupt);
    }
    request.command.require_current_authority()?;
    *state = candidate;
    Ok(QueuedCancellationOutcome::Applied(Box::new(result)))
}
