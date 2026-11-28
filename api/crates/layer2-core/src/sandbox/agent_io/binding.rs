use crate::sandbox::store::AgentConversationMutation;
use crate::sandbox::{
    Agent, AgentLogKind, AgentOperationDelivery, AgentOperationIntent, AgentStatus,
};

use super::{
    AdmittedAgentCommand, AgentCommand, AgentInputOrder, AgentOwner, AgentResponseStatus,
    CommandDispatch, CommandLane, CommandLedgerError,
};

/// Called only inside the Agent/log transaction, with the same snapshot's lane,
/// command and latest turn. A persisted binding is never eligible for dispatch again.
pub fn bind_command_dispatch(
    command: &mut AdmittedAgentCommand,
    lane: &CommandLane,
    current: &Agent,
    latest_turn: Option<u64>,
    mutation: &AgentConversationMutation,
    now: &str,
) -> Result<CommandDispatch, CommandLedgerError> {
    if command
        .receipt
        .envelope
        .authorization
        .authority_id
        .starts_with("binding:")
        && mutation.execution_fence.is_none()
    {
        return Err(CommandLedgerError::Conflict);
    }
    let claim = mutation
        .command_claim
        .as_ref()
        .ok_or(CommandLedgerError::Invalid)?;
    lane.require_claim(claim, now)?;
    command.require_claim(claim, now)?;
    if lane.lease != command.work.lease || command.receipt.agent_order > lane.last_order {
        return Err(CommandLedgerError::Corrupt);
    }
    if let Some(fence) = &mutation.execution_fence {
        fence
            .require_envelope(&command.receipt.envelope)
            .map_err(|_| CommandLedgerError::Conflict)?;
    } else if let Some(fence) = &mutation.trigger_execution_fence {
        fence
            .require_dispatch(command)
            .map_err(|_| CommandLedgerError::Conflict)?;
        if !super::is_bound_trigger_first(command, current) {
            return Err(CommandLedgerError::Conflict);
        }
    } else if command.receipt.envelope.is_first_message()
        || matches!(
            command.receipt.envelope.actor,
            super::AgentActor::Channel { .. }
        )
        || command.receipt.envelope.source.kind == super::AgentSourceKind::Channel
        || command.receipt.envelope.owner
            != (AgentOwner::Person {
                subject: current.owner.clone(),
            })
    {
        return Err(CommandLedgerError::Conflict);
    }
    if mutation.admission_guard.is_none()
        || command.predates_owner_restart(current)
        || current.work.step.fences_conversation_output()
        || command.status != AgentResponseStatus::Queued
        || command.work.dispatch.is_some()
        || command.queue_expired(now)
        || mutation.agent.id != current.id
        || command.receipt.envelope.agent_id != current.id
        || current.requested_stop_reason.is_some()
        || current
            .conversation
            .operation
            .as_ref()
            .is_some_and(|op| op.blocks_dispatch())
    {
        return Err(CommandLedgerError::Conflict);
    }
    let operation = mutation
        .agent
        .conversation
        .operation
        .as_ref()
        .ok_or(CommandLedgerError::Conflict)?;
    operation
        .validate()
        .map_err(|_| CommandLedgerError::Corrupt)?;
    if operation.sequence
        != current
            .conversation
            .last_seq
            .checked_add(1)
            .ok_or(CommandLedgerError::Corrupt)?
        || operation.runtime_generation != mutation.runtime_generation
        || operation.state != AgentOperationDelivery::DispatchFenced
        || operation.dispatch_fenced_at != mutation.appended_at
        || mutation.agent.conversation.active_turn != Some(operation.turn_sequence)
        || mutation.agent_logs.first()
            != Some(&AgentLogKind::OperationDeliveryChanged {
                operation_sequence: operation.sequence,
                operation: operation.intent.kind(),
                state: AgentOperationDelivery::DispatchFenced,
            })
    {
        return Err(CommandLedgerError::Conflict);
    }
    let mut expected_metadata = Vec::new();
    let effective = command.effective_command()?;
    let previous_task = &current.conversation.observations.task_acceptance;
    let task = if let Some(intent) = effective.task_acceptance() {
        Some(
            crate::sandbox::task_acceptance::TaskAcceptance::bind(
                previous_task.as_ref(),
                intent,
                &claim.command_id,
                operation.turn_sequence,
            )
            .map_err(|_| CommandLedgerError::Conflict)?,
        )
    } else {
        previous_task.clone()
    };
    if mutation.agent.conversation.observations.task_acceptance != task {
        return Err(CommandLedgerError::Conflict);
    }
    let (intent, text) = match &effective {
        AgentCommand::Message {
            input,
            order,
            metadata,
        } => {
            if current.status != AgentStatus::Ready
                || current.conversation.active_turn.is_some()
                || current
                    .conversation
                    .queued_messages
                    .iter()
                    .any(|message| message.state != crate::sandbox::QueuedMessageState::Delivered)
                || current
                    .conversation
                    .first_message
                    .as_ref()
                    .is_some_and(|first| {
                        !(first.is_terminal()
                            || mutation.trigger_execution_fence.is_some()
                                && super::is_bound_trigger_first(command, current))
                    })
                || command.target_turn.is_some()
                || operation.turn_sequence
                    != operation
                        .sequence
                        .checked_add(1)
                        .ok_or(CommandLedgerError::Corrupt)?
                || matches!(order, AgentInputOrder::AfterTurn { turn_sequence } if *turn_sequence != latest_turn)
                || mutation.agent_logs.get(1)
                    != Some(&AgentLogKind::TurnStarted {
                        model: Some(current.settings().model),
                        turn_sequence: operation.turn_sequence,
                        origin_key: Some(format!("command:{}", claim.command_id.as_str())),
                    })
            {
                return Err(CommandLedgerError::Conflict);
            }
            if !metadata.is_empty() {
                expected_metadata.push(AgentLogKind::TurnMetadataSet {
                    turn_sequence: operation.turn_sequence,
                    metadata: metadata.clone(),
                });
            }
            let text = input
                .clone()
                .into_text()
                .map_err(|_| CommandLedgerError::NotAdmissible)?;
            (AgentOperationIntent::send_message(&text), Some(text))
        }
        control => {
            if command.target_turn.is_none()
                || command.target_turn != current.conversation.active_turn
                || command.target_turn != Some(operation.turn_sequence)
                || mutation
                    .agent_logs
                    .iter()
                    .any(|kind| matches!(kind, AgentLogKind::TurnStarted { .. }))
            {
                return Err(CommandLedgerError::Conflict);
            }
            match control {
                AgentCommand::Steer { input } => {
                    let text = input
                        .clone()
                        .into_text()
                        .map_err(|_| CommandLedgerError::NotAdmissible)?;
                    (AgentOperationIntent::steer(&text), Some(text))
                }
                AgentCommand::Interrupt => (AgentOperationIntent::Interrupt, None),
                AgentCommand::AnswerApproval {
                    approval_id,
                    decision,
                } => {
                    if !mutation
                        .agent_logs
                        .contains(&AgentLogKind::ApprovalResolved {
                            approval_id: approval_id.clone(),
                            decision: decision.as_str().into(),
                        })
                    {
                        return Err(CommandLedgerError::Conflict);
                    }
                    (
                        AgentOperationIntent::answer_approval(approval_id.clone(), *decision)
                            .map_err(|_| CommandLedgerError::Corrupt)?,
                        None,
                    )
                }
                AgentCommand::Message { .. } => unreachable!(),
            }
        }
    };
    let actual_text: Vec<_> = mutation
        .agent_logs
        .iter()
        .filter_map(|kind| match kind {
            AgentLogKind::OwnerMessage { content } => Some(content),
            _ => None,
        })
        .collect();
    let actual_metadata: Vec<_> = mutation
        .agent_logs
        .iter()
        .filter(|kind| matches!(kind, AgentLogKind::TurnMetadataSet { .. }))
        .collect();
    if operation.intent != intent
        || actual_text != text.iter().collect::<Vec<_>>()
        || actual_metadata != expected_metadata.iter().collect::<Vec<_>>()
    {
        return Err(CommandLedgerError::Conflict);
    }
    let dispatch = CommandDispatch {
        claim: claim.clone(),
        operation_sequence: operation.sequence,
        turn_sequence: operation.turn_sequence,
        runtime_generation: operation.runtime_generation,
        fenced_at: operation.dispatch_fenced_at.clone(),
        delivery: operation.state,
    };
    command.revision = command
        .revision
        .checked_add(1)
        .ok_or(CommandLedgerError::Corrupt)?;
    command.status = AgentResponseStatus::InProgress;
    command.work.dispatch = Some(dispatch.clone());
    command.validate()?;
    Ok(dispatch)
}
