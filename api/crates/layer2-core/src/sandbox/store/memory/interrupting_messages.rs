use crate::sandbox::agent_io::{
    validate_command_reservation_policy, AdmittedInterruptingMessage, CommandLedgerError,
    CommandReservationResult, InterruptingMessageReservation,
};
use crate::sandbox::store::InMemorySandboxStore;

pub(super) fn reserve(
    store: &InMemorySandboxStore,
    request: InterruptingMessageReservation,
) -> Result<AdmittedInterruptingMessage, CommandLedgerError> {
    request.validate()?;
    let mut state = store.lock();
    let control_key = (
        request.control.command.scope(),
        request.control.command.key_sha256.clone(),
    );
    let message_key = (request.message.scope(), request.message.key_sha256.clone());
    let existing = match (
        state.agent_command_keys.get(&control_key),
        state.agent_command_keys.get(&message_key),
    ) {
        (Some(control), Some(message)) => Some(
            request.replay_records(
                state
                    .agent_commands
                    .get(control)
                    .cloned()
                    .ok_or(CommandLedgerError::Corrupt)?,
                state.targeted_controls.get(control).cloned(),
                state
                    .agent_commands
                    .get(message)
                    .cloned()
                    .ok_or(CommandLedgerError::Corrupt)?,
            )?,
        ),
        (None, None) => None,
        _ => return Err(CommandLedgerError::IdempotencyConflict),
    };
    let agent = state
        .agents
        .get(&request.control.command.envelope.agent_id)
        .ok_or(CommandLedgerError::NotFound)?;
    let parent = state
        .sandboxes
        .get(&agent.sandbox_id)
        .ok_or(CommandLedgerError::NotFound)?;
    request.validate_current(agent, parent)?;
    for reservation in [&request.control.command, &request.message] {
        super::bindings::require_conversation_authority(&state, reservation.binding.as_ref())
            .map_err(|_| CommandLedgerError::Conflict)?;
        validate_command_reservation_policy(
            reservation,
            parent,
            state.resource_policies.get(&parent.id),
        )?;
    }
    let mut sources = state.agent_commands.values().filter(|record| {
        record.receipt.response_id.as_ref() == Some(&request.control.target.response_id)
    });
    let source = sources
        .next()
        .cloned()
        .ok_or(CommandLedgerError::NotFound)?;
    if sources.next().is_some() {
        return Err(CommandLedgerError::Corrupt);
    }
    request.control.validate_source(agent, &source)?;
    if let Some(existing) = existing {
        existing
            .control
            .binding
            .validate_response(&existing.control.record, &source)?;
        request.require_current_authority()?;
        return Ok(existing);
    }
    request.control.validate_target(agent, &source)?;
    let mut candidate = state.clone();
    let CommandReservationResult::Created(control) =
        super::agent_commands::reserve(&mut candidate, &request.control.command)?
    else {
        return Err(CommandLedgerError::IdempotencyConflict);
    };
    let control = request.control.bind(*control, &source)?;
    if candidate
        .targeted_controls
        .insert(control.binding.command_id.clone(), control.binding.clone())
        .is_some()
    {
        return Err(CommandLedgerError::Corrupt);
    }
    let CommandReservationResult::Created(message) =
        super::agent_commands::reserve(&mut candidate, &request.message)?
    else {
        return Err(CommandLedgerError::IdempotencyConflict);
    };
    let result = request.replay(control, *message)?;
    request.require_current_authority()?;
    *state = candidate;
    Ok(result)
}
