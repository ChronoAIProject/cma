use crate::sandbox::agent_io::{
    validate_command_reservation_policy, AdmittedAgentCommand, AdmittedTargetedControl,
    AgentCommand, CommandLedgerError, CommandReservationResult, TargetedControlInspection,
    TargetedControlReservation,
};
use crate::sandbox::store::{InMemorySandboxStore, State};
use crate::sandbox::{Agent, PendingAgentApprovals};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Lookup,
    Reserve,
    Inspect,
}

pub(super) fn apply(
    store: &InMemorySandboxStore,
    request: TargetedControlReservation,
    create: bool,
) -> Result<Option<AdmittedTargetedControl>, CommandLedgerError> {
    let mode = if create { Mode::Reserve } else { Mode::Lookup };
    Ok(match execute(store, request, mode)? {
        TargetedControlInspection::Existing(existing) => Some(*existing),
        TargetedControlInspection::Missing | TargetedControlInspection::NotAdmitted => None,
    })
}

pub(super) fn inspect(
    store: &InMemorySandboxStore,
    request: TargetedControlReservation,
) -> Result<TargetedControlInspection, CommandLedgerError> {
    execute(store, request, Mode::Inspect)
}

fn execute(
    store: &InMemorySandboxStore,
    request: TargetedControlReservation,
    mode: Mode,
) -> Result<TargetedControlInspection, CommandLedgerError> {
    request.validate()?;
    let mut state = store.lock();
    super::bindings::require_conversation_authority(&state, request.command.binding.as_ref())
        .map_err(|_| CommandLedgerError::Conflict)?;
    request.command.require_current_authority()?;
    let key = (request.command.scope(), request.command.key_sha256.clone());
    let existing = state
        .agent_command_keys
        .get(&key)
        .map(|id| {
            let record = state
                .agent_commands
                .get(id)
                .ok_or(CommandLedgerError::Corrupt)?;
            request.replay(record.clone(), state.targeted_controls.get(id).cloned())
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
        let response = state
            .agent_commands
            .get(&existing.binding.response_command_id)
            .ok_or(CommandLedgerError::Corrupt)?;
        existing
            .binding
            .validate_response(&existing.record, response)?;
        request.command.require_current_authority()?;
        return Ok(TargetedControlInspection::Existing(Box::new(existing)));
    }
    if mode != Mode::Reserve {
        if mode == Mode::Inspect && request.command.expected_agent_revision < agent.revision {
            request.validate_source(agent, &response(&state, &request)?)?;
            request.command.require_current_authority()?;
            // The same lock fences admission and resource mutations. No old
            // revision can reserve after this atomic absence observation.
            return Ok(TargetedControlInspection::NotAdmitted);
        }
        return Ok(TargetedControlInspection::Missing);
    }
    let response = response(&state, &request)?;
    request.validate_target(agent, &response)?;
    if matches!(
        request.command.envelope.command,
        AgentCommand::AnswerApproval { .. }
    ) {
        request.validate_approval(&pending(&state, agent)?)?;
    }
    // Match Mongo rollback: even an audit or sidecar failure after staging the
    // ordinary command leaves no key, order, audit or partial control behind.
    let mut candidate = state.clone();
    let CommandReservationResult::Created(record) =
        super::agent_commands::reserve(&mut candidate, &request.command)?
    else {
        return Err(CommandLedgerError::IdempotencyConflict);
    };
    let result = request.bind(*record, &response)?;
    if candidate
        .targeted_controls
        .insert(result.binding.command_id.clone(), result.binding.clone())
        .is_some()
    {
        return Err(CommandLedgerError::Corrupt);
    }
    request.command.require_current_authority()?;
    *state = candidate;
    Ok(TargetedControlInspection::Existing(Box::new(result)))
}

fn response(
    state: &State,
    request: &TargetedControlReservation,
) -> Result<AdmittedAgentCommand, CommandLedgerError> {
    let mut matches = state
        .agent_commands
        .values()
        .filter(|record| record.receipt.response_id.as_ref() == Some(&request.target.response_id));
    let value = matches.next().ok_or(CommandLedgerError::NotFound)?;
    if matches.next().is_some() {
        return Err(CommandLedgerError::Corrupt);
    }
    Ok(value.clone())
}

fn pending(
    state: &State,
    agent: &Agent,
) -> Result<Vec<crate::sandbox::AgentLogEntry>, CommandLedgerError> {
    let entries = state
        .agent_logs
        .get(&agent.id)
        .map(Vec::as_slice)
        .unwrap_or_default();
    if entries.len() as u64 != agent.conversation.last_seq {
        return Err(CommandLedgerError::Corrupt);
    }
    let mut pending = PendingAgentApprovals::default();
    for (index, entry) in entries.iter().enumerate() {
        if entry.agent_id != agent.id || entry.seq != index as u64 + 1 {
            return Err(CommandLedgerError::Corrupt);
        }
        pending.observe(entry);
    }
    Ok(pending.into_entries())
}
