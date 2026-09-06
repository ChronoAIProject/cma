use super::{
    Agent, AgentLogKind, AgentOperationFailure, AgentStatus, AgentStopReason, AgentWorkStep,
    FirstMessageDelivery, QueuedMessageState, Sandbox, SandboxModelError,
};

/// Derive one recovery mutation without replacing any durable conversation log.
pub(super) fn recover_agent(
    sandbox: &Sandbox,
    current: &Agent,
    at: &str,
) -> Result<(Agent, Vec<AgentLogKind>), SandboxModelError> {
    current.validate_conversation()?;
    let covered = current.status != AgentStatus::Starting
        && current.requested_stop_reason.is_none()
        && sandbox
            .lifecycle
            .checkpoints
            .latest
            .as_ref()
            .is_some_and(|checkpoint| {
                checkpoint.conversations.iter().any(|conversation| {
                    conversation.agent_id == current.id
                        && Some(conversation.conversation_id.as_str())
                            == current.conversation.conversation_id.as_deref()
                })
            });
    let mut next = current.clone();
    let mut logs = Vec::new();
    if let Some(turn_sequence) = next.conversation.active_turn.take() {
        logs.push(AgentLogKind::TurnInterrupted { turn_sequence });
    }
    if let Some(operation) = next.conversation.operation.as_mut() {
        if operation.blocks_dispatch() {
            operation.retire_lost_runtime(AgentOperationFailure::new(
                "runtime_lost",
                "the runtime was lost before the message write outcome could be determined",
                at,
            )?)?;
            logs.push(AgentLogKind::OperationDeliveryChanged {
                operation_sequence: operation.sequence,
                operation: operation.intent.kind(),
                state: operation.state,
            });
        }
    }
    if let Some(first) = next.conversation.first_message.as_mut() {
        if !first.is_terminal() && (!covered || first.state != FirstMessageDelivery::Queued) {
            first.fail_runtime_loss(at.into())?;
            logs.push(AgentLogKind::FirstMessageChanged {
                state: first.state,
                recovery_generation: first.recovery_generation,
            });
        }
    }
    // Already attempted messages are retained in owner logs, never replayed from the queue.
    next.conversation
        .queued_messages
        .retain(|message| message.state == QueuedMessageState::Queued);
    if covered {
        let previous_generation = current
            .conversation
            .runtime_generation
            .ok_or_else(|| SandboxModelError::invalid_value("agent.runtime_generation"))?;
        let runtime_generation = previous_generation
            .checked_add(1)
            .ok_or_else(|| SandboxModelError::invalid_value("agent.runtime_generation"))?;
        next.conversation.runtime_generation = Some(runtime_generation);
        logs.push(AgentLogKind::RuntimeGenerationRetired {
            previous_generation,
            runtime_generation,
        });
        if next.status != AgentStatus::Paused {
            next.transition(AgentStatus::Paused, at.into(), None)?;
        }
    } else {
        let reason = next
            .requested_stop_reason
            .clone()
            .unwrap_or(AgentStopReason::RuntimeLost);
        next.transition(AgentStatus::Stopped, at.into(), Some(reason.clone()))?;
        logs.push(AgentLogKind::Stopped {
            reason: reason.as_str().into(),
        });
    }
    if next.status != current.status {
        logs.push(AgentLogKind::StatusChanged {
            from: current.status,
            to: next.status,
        });
    }
    next.work.schedule(AgentWorkStep::None, at.into());
    next.work.receipts.clear();
    next.work.cleanup_debt.clear();
    next.updated_at = at.into();
    next.revision = current
        .revision
        .checked_add(1)
        .ok_or_else(|| SandboxModelError::invalid_value("agent.revision"))?;
    next.conversation.last_seq = current
        .conversation
        .last_seq
        .checked_add(u64::try_from(logs.len()).map_err(|_| invalid_sequence())?)
        .ok_or_else(invalid_sequence)?;
    next.validate_conversation()?;
    Ok((next, logs))
}

const fn invalid_sequence() -> SandboxModelError {
    SandboxModelError::invalid_value("agent.conversation.last_seq")
}
