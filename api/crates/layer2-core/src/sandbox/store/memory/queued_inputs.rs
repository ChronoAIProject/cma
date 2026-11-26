use crate::sandbox::agent_io::{
    validate_command_reservation_policy, AgentResponseStatus, CommandAuthorizationAudit,
    CommandAuthorizationEffect, CommandLedgerError, QueuedInputOutcome, QueuedInputReservation,
};
use crate::sandbox::store::InMemorySandboxStore;

pub(super) fn apply(
    store: &InMemorySandboxStore,
    request: QueuedInputReservation,
) -> Result<QueuedInputOutcome, CommandLedgerError> {
    request.validate()?;
    let mut state = store.lock();
    super::bindings::require_conversation_authority(
        &state,
        request.cancellation.command.binding.as_ref(),
    )
    .map_err(|_| CommandLedgerError::Conflict)?;
    let key = (
        request.cancellation.command.scope(),
        request.cancellation.command.key_sha256.clone(),
    );
    let existing = state
        .agent_command_keys
        .get(&key)
        .map(|id| {
            let record = state
                .agent_commands
                .get(id)
                .ok_or(CommandLedgerError::Corrupt)?;
            request.cancellation.command.replay(record.clone())?;
            let binding = state
                .queued_input_edits
                .get(id)
                .ok_or(CommandLedgerError::IdempotencyConflict)?;
            if binding.request_fingerprint != request.fingerprint()? {
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
        .get(&request.cancellation.command.envelope.agent_id)
        .ok_or(CommandLedgerError::NotFound)?;
    let parent = state
        .sandboxes
        .get(&agent.sandbox_id)
        .ok_or(CommandLedgerError::NotFound)?;
    request
        .cancellation
        .validate_current_identity(agent, parent)?;
    if !request
        .cancellation
        .current_observation_matches(agent, parent)
    {
        return Ok(QueuedInputOutcome::StaleObservation);
    }
    validate_command_reservation_policy(
        &request.cancellation.command,
        parent,
        state.resource_policies.get(&parent.id),
    )?;
    if let Some(existing) = existing {
        request.cancellation.command.require_current_authority()?;
        return Ok(QueuedInputOutcome::Applied(Box::new(existing)));
    }
    let mut response = state
        .agent_commands
        .get(&request.cancellation.target.source_command_id)
        .cloned()
        .ok_or(CommandLedgerError::NotFound)?;
    request.cancellation.validate_source(agent, &response)?;
    if request.cancellation.target.expected_command_revision < response.revision {
        request.cancellation.command.require_current_authority()?;
        return Ok(QueuedInputOutcome::NotAdmitted);
    }
    let mut lane = state
        .agent_command_orders
        .get(&agent.id)
        .cloned()
        .ok_or(CommandLedgerError::Corrupt)?;
    if lane.last_order < response.receipt.agent_order {
        return Err(CommandLedgerError::Corrupt);
    }
    request.revise(&mut response, &mut lane)?;
    lane.last_order = lane
        .last_order
        .checked_add(1)
        .filter(|order| *order <= i64::MAX as u64)
        .ok_or(CommandLedgerError::Corrupt)?;
    let mut record =
        request
            .cancellation
            .command
            .record(lane.last_order, agent.conversation.last_seq, None)?;
    record.status = AgentResponseStatus::Completed;
    let result = request.bind(record, response)?;
    let id = result.record.receipt.envelope.command_id.clone();
    if state.agent_commands.contains_key(&id) {
        return Err(CommandLedgerError::Conflict);
    }
    let mut candidate = state.clone();
    if let Some(evidence) = &request.cancellation.command.admission_audit {
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
        .insert(request.cancellation.command.envelope.agent_id.clone(), lane);
    candidate.agent_commands.insert(
        result.binding.target.source_command_id.clone(),
        result.response.clone(),
    );
    candidate.agent_command_keys.insert(key, id.clone());
    candidate
        .agent_commands
        .insert(id.clone(), result.record.clone());
    if candidate
        .queued_input_edits
        .insert(id, result.binding.clone())
        .is_some()
    {
        return Err(CommandLedgerError::Corrupt);
    }
    request.cancellation.command.require_current_authority()?;
    *state = candidate;
    Ok(QueuedInputOutcome::Applied(Box::new(result)))
}
