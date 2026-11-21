use super::TriggerExecutionAuthorization;
use crate::sandbox::{
    Agent, AgentLogKind, AgentOperationDelivery, AgentOperationError, FirstMessageDelivery,
    FirstMessageFailure,
};

pub(in crate::sandbox) fn fence_first(
    authority: &TriggerExecutionAuthorization,
    current: &Agent,
    next: &mut Agent,
    claim_fence: u64,
    at: &str,
    logs: &mut Vec<AgentLogKind>,
) -> Result<(), AgentOperationError> {
    authority
        .fence()
        .binding()
        .require_agent(current)
        .map_err(|_| AgentOperationError::Forbidden)?;
    let operation = next
        .conversation
        .operation
        .as_ref()
        .ok_or(AgentOperationError::Conflict)?;
    let first = next
        .conversation
        .first_message
        .as_mut()
        .ok_or(AgentOperationError::Conflict)?;
    first
        .fence_dispatch_for(
            operation.turn_sequence,
            operation.sequence,
            claim_fence,
            at.into(),
        )
        .map_err(|error| AgentOperationError::Corrupt(error.to_string()))?;
    logs.push(AgentLogKind::FirstMessageChanged {
        state: first.state,
        recovery_generation: first.recovery_generation,
    });
    Ok(())
}

pub(in crate::sandbox) fn project_delivery(
    agent: &mut Agent,
    operation_sequence: u64,
    at: &str,
    logs: &mut Vec<AgentLogKind>,
) -> Result<(), AgentOperationError> {
    let Some(first) = agent
        .conversation
        .first_message
        .as_mut()
        .filter(|first| first.common_binding.is_some())
    else {
        return Ok(());
    };
    if first.fence_sequence != operation_sequence
        || first.is_terminal()
        || first.state == FirstMessageDelivery::ExecutionObserved
    {
        return Ok(());
    }
    let operation = agent
        .conversation
        .operation
        .as_ref()
        .ok_or(AgentOperationError::Conflict)?;
    if operation.sequence != operation_sequence
        || first.turn_sequence != Some(operation.turn_sequence)
    {
        return Err(AgentOperationError::Conflict);
    }
    let previous = first.state;
    let detail = operation
        .failure
        .as_ref()
        .map_or("common Trigger input delivery failed", |failure| {
            failure.detail.as_str()
        });
    let result = match operation.state {
        AgentOperationDelivery::WriteConfirmed => first.mark_write_confirmed(at.into()),
        AgentOperationDelivery::ExecutionObserved => first.mark_execution_observed(at.into()),
        AgentOperationDelivery::PreWriteFailed => first.fail(
            FirstMessageFailure::new("trigger_command_pre_write_failed", detail, false, at)
                .map_err(|error| AgentOperationError::Corrupt(error.to_string()))?,
        ),
        AgentOperationDelivery::RecoveryRequired => first.require_recovery(
            FirstMessageFailure::new("trigger_command_write_ambiguous", detail, false, at)
                .map_err(|error| AgentOperationError::Corrupt(error.to_string()))?,
        ),
        AgentOperationDelivery::RuntimeLost => first.fail(
            FirstMessageFailure::new("trigger_command_runtime_lost", detail, false, at)
                .map_err(|error| AgentOperationError::Corrupt(error.to_string()))?,
        ),
        AgentOperationDelivery::DispatchFenced => Ok(()),
    };
    result.map_err(|error| AgentOperationError::Corrupt(error.to_string()))?;
    if previous != first.state {
        logs.push(AgentLogKind::FirstMessageChanged {
            state: first.state,
            recovery_generation: first.recovery_generation,
        });
    }
    Ok(())
}
