mod queue_delivery;
use queue_delivery::{latest_delivery_is_durable, validate_queue_prune};

use std::collections::BTreeSet;

use crate::sandbox::queued_message::queued_message_origin;
use crate::sandbox::{
    Agent, AgentLogKind, AgentOperationDelivery, AgentOperationIntent, AgentOperationRecord,
    AgentStatus, QueuedMessageState, Sandbox, SandboxImageRotationPhase, SandboxLifecycleOperation,
    SandboxPausePhase, SandboxRevivePhase, SandboxStatus,
};

use super::lifecycle_event_validation::validate_lifecycle_events;
use super::lifecycle_queue_recovery_validation::validate_queue_recovery;
use super::{
    SandboxLifecycleAgentMutation, SandboxLifecycleAgentScope, SandboxLifecycleMutation,
    SandboxStoreError,
};

#[derive(Clone, Copy)]
pub(super) enum AgentPhase {
    Pause,
    Reopen,
    QueueProgress,
    QueueRecovery,
    QueueCompletion,
    SandboxOnly,
}

pub(super) fn validate_lifecycle_agent_phase(
    current: &Sandbox,
    agents: &[(Agent, u64)],
    mutation: &SandboxLifecycleMutation,
    touched: &BTreeSet<crate::AgentId>,
) -> Result<(), SandboxStoreError> {
    for change in &mutation.agents {
        if agents.iter().any(|(agent, _)| {
            agent.id == change.agent.id
                && agent.conversation.settings != change.agent.conversation.settings
        }) {
            return Err(invalid(
                "lifecycle cannot change owner conversation settings",
            ));
        }
    }
    let phase = agent_phase(current, &mutation.sandbox, mutation.agents.is_empty());
    match phase {
        AgentPhase::Pause => {
            if mutation.agent_scope != SandboxLifecycleAgentScope::AllLive {
                return Err(invalid("pause requires the exact live-agent scope"));
            }
            validate_all_live_scope(current, agents, mutation, touched)?;
        }
        AgentPhase::Reopen => validate_all_live_scope(current, agents, mutation, touched)?,
        AgentPhase::QueueProgress => {
            if mutation.agent_scope != SandboxLifecycleAgentScope::Touched
                || mutation.agents.len() != 1
            {
                return Err(invalid("queue progress must fence exactly one agent"));
            }
        }
        AgentPhase::QueueRecovery => {
            if mutation.agent_scope != SandboxLifecycleAgentScope::Touched
                || mutation.agents.len() != 1
            {
                return Err(invalid("queue recovery must retire exactly one fence"));
            }
        }
        AgentPhase::QueueCompletion => {
            validate_queue_completion_set(current, agents, mutation, touched)?;
        }
        AgentPhase::SandboxOnly => {
            if mutation.agent_scope != SandboxLifecycleAgentScope::Touched
                || !mutation.agents.is_empty()
            {
                return Err(invalid("sandbox-only lifecycle phase changed agents"));
            }
            if mutation.sandbox.status == SandboxStatus::Active
                || (current.retained_home_recovery().is_some()
                    && mutation.sandbox.status == SandboxStatus::Pausing)
            {
                validate_final_publication_agents(current, agents)?;
            }
        }
    }

    for change in &mutation.agents {
        let stored = agents
            .iter()
            .find_map(|(agent, _)| (agent.id == change.agent.id).then_some(agent))
            .ok_or(SandboxStoreError::NotFound)?;
        match phase {
            AgentPhase::Pause => validate_pause(stored, change)?,
            AgentPhase::Reopen => validate_reopen(current, stored, change)?,
            AgentPhase::QueueProgress => {
                validate_queue_progress(stored, change, &mutation.appended_at)?
            }
            AgentPhase::QueueRecovery => {
                validate_queue_recovery(stored, change, &mutation.appended_at)?
            }
            AgentPhase::QueueCompletion => validate_queue_prune(stored, change)?,
            AgentPhase::SandboxOnly => unreachable!("sandbox-only phases have no agent changes"),
        }
    }
    validate_lifecycle_events(current, mutation, phase)
}

fn agent_phase(current: &Sandbox, next: &Sandbox, no_agents: bool) -> AgentPhase {
    if no_agents
        && (next.status == SandboxStatus::Failed
            || next
                .runtime_update_request()
                .is_some_and(|request| request.failure.is_some())
            || next.work.attempts == current.work.attempts.saturating_add(1))
    {
        return AgentPhase::SandboxOnly;
    }
    match current.lifecycle.operation.as_ref() {
        Some(SandboxLifecycleOperation::Pause {
            phase: SandboxPausePhase::PauseAgents,
            ..
        })
        | Some(SandboxLifecycleOperation::ImageRotation {
            phase: SandboxImageRotationPhase::PauseAgents,
            ..
        }) => AgentPhase::Pause,
        Some(SandboxLifecycleOperation::Revive {
            phase: SandboxRevivePhase::ConversationReopen,
            ..
        })
        | Some(SandboxLifecycleOperation::ImageRotation {
            phase: SandboxImageRotationPhase::ConversationReopen,
            ..
        }) => AgentPhase::Reopen,
        Some(SandboxLifecycleOperation::Revive {
            phase: SandboxRevivePhase::QueuedMessageDelivery,
            ..
        })
        | Some(SandboxLifecycleOperation::ImageRotation {
            phase: SandboxImageRotationPhase::QueuedMessageDelivery,
            ..
        }) if current.lifecycle.operation == next.lifecycle.operation
            && next.work.attempts == current.work.attempts.saturating_add(1)
            && !no_agents =>
        {
            AgentPhase::QueueRecovery
        }
        Some(SandboxLifecycleOperation::Revive {
            phase: SandboxRevivePhase::QueuedMessageDelivery,
            ..
        })
        | Some(SandboxLifecycleOperation::ImageRotation {
            phase: SandboxImageRotationPhase::QueuedMessageDelivery,
            ..
        }) if current.lifecycle.operation == next.lifecycle.operation && !no_agents => {
            AgentPhase::QueueProgress
        }
        Some(SandboxLifecycleOperation::Revive {
            phase: SandboxRevivePhase::QueuedMessageDelivery,
            ..
        })
        | Some(SandboxLifecycleOperation::ImageRotation {
            phase: SandboxImageRotationPhase::QueuedMessageDelivery,
            ..
        }) if current.lifecycle.operation != next.lifecycle.operation => {
            AgentPhase::QueueCompletion
        }
        _ => AgentPhase::SandboxOnly,
    }
}

fn validate_all_live_scope(
    sandbox: &Sandbox,
    agents: &[(Agent, u64)],
    mutation: &SandboxLifecycleMutation,
    touched: &BTreeSet<crate::AgentId>,
) -> Result<(), SandboxStoreError> {
    let live = live_ids(sandbox, agents);
    if mutation.agent_scope == SandboxLifecycleAgentScope::Touched || *touched != live {
        return Err(invalid("lifecycle phase requires the exact live-agent set"));
    }
    Ok(())
}

fn validate_pause(
    current: &Agent,
    change: &SandboxLifecycleAgentMutation,
) -> Result<(), SandboxStoreError> {
    let next = &change.agent;
    let expected_logs = (current.status != AgentStatus::Paused)
        .then_some(AgentLogKind::StatusChanged {
            from: current.status,
            to: AgentStatus::Paused,
        })
        .into_iter()
        .collect::<Vec<_>>();
    if current.conversation.active_turn.is_some()
        || blocking_operation(current)
        || next.status != AgentStatus::Paused
        || !conversation_matches_except_generation(current, next)
        || current.conversation.runtime_generation != next.conversation.runtime_generation
        || change.logs != expected_logs
    {
        return Err(invalid("pause-agents mutation is not exact"));
    }
    Ok(())
}

fn validate_reopen(
    sandbox: &Sandbox,
    current: &Agent,
    change: &SandboxLifecycleAgentMutation,
) -> Result<(), SandboxStoreError> {
    let next = &change.agent;
    let Some(generation) = next.conversation.runtime_generation else {
        return Err(invalid("conversation reopen has no runtime generation"));
    };
    let expected_logs = vec![
        AgentLogKind::ConversationOpened {
            conversation_id: next
                .conversation
                .conversation_id
                .clone()
                .ok_or_else(|| invalid("conversation reopen has no identity"))?,
            runtime_generation: generation,
        },
        AgentLogKind::StatusChanged {
            from: AgentStatus::Paused,
            to: AgentStatus::Ready,
        },
    ];
    if sandbox
        .runtime
        .as_ref()
        .and_then(|binding| binding.provision_generation)
        != Some(generation)
        || current.status != AgentStatus::Paused
        || current.conversation.active_turn.is_some()
        || blocking_operation(current)
        || next.status != AgentStatus::Ready
        || current.conversation.conversation_id != next.conversation.conversation_id
        || current
            .conversation
            .runtime_generation
            .is_none_or(|current| generation <= current)
        || current.conversation.first_message != next.conversation.first_message
        || current.conversation.active_turn != next.conversation.active_turn
        || current.conversation.operation != next.conversation.operation
        || current.conversation.queued_messages != next.conversation.queued_messages
        || change.logs != expected_logs
    {
        return Err(invalid("conversation-reopen mutation is not exact"));
    }
    Ok(())
}

fn validate_queue_progress(
    current: &Agent,
    change: &SandboxLifecycleAgentMutation,
    at: &str,
) -> Result<(), SandboxStoreError> {
    let next = &change.agent;
    let operation_sequence = current
        .conversation
        .last_seq
        .checked_add(1)
        .ok_or(SandboxStoreError::SequenceConflict)?;
    let turn_sequence = operation_sequence
        .checked_add(1)
        .ok_or(SandboxStoreError::SequenceConflict)?;
    let (queued_sequence, payload_sha256) = exact_queue_fence(current, next, at)?;
    let runtime_generation = current
        .conversation
        .runtime_generation
        .ok_or_else(|| invalid("queue dispatch has no runtime generation"))?;
    let expected_operation = AgentOperationRecord::new(
        operation_sequence,
        runtime_generation,
        turn_sequence,
        AgentOperationIntent::SendMessage { payload_sha256 },
        at.into(),
    )
    .map_err(|_| invalid("queue dispatch operation is invalid"))?;
    let expected_logs = vec![
        AgentLogKind::OperationDeliveryChanged {
            operation_sequence,
            operation: crate::sandbox::AgentOperationKind::SendMessage,
            state: AgentOperationDelivery::DispatchFenced,
        },
        AgentLogKind::TurnStarted {
            model: Some(current.settings().model),
            turn_sequence,
            origin_key: Some(queued_message_origin(queued_sequence)),
        },
        AgentLogKind::StatusChanged {
            from: AgentStatus::Ready,
            to: AgentStatus::Working,
        },
    ];
    if current.status != AgentStatus::Ready
        || current.conversation.active_turn.is_some()
        || blocking_operation(current)
        || current
            .conversation
            .first_message
            .as_ref()
            .is_some_and(|message| !message.is_terminal())
        || next.status != AgentStatus::Working
        || next.conversation.conversation_id != current.conversation.conversation_id
        || next.conversation.runtime_generation != current.conversation.runtime_generation
        || next.conversation.first_message != current.conversation.first_message
        || next.conversation.active_turn != Some(turn_sequence)
        || next.conversation.operation.as_ref() != Some(&expected_operation)
        || change.logs != expected_logs
    {
        return Err(invalid("queue-progress agent mutation is not exact"));
    }
    Ok(())
}

fn exact_queue_fence(
    current: &Agent,
    next: &Agent,
    at: &str,
) -> Result<(u64, String), SandboxStoreError> {
    if current.conversation.queued_messages.len() != next.conversation.queued_messages.len() {
        return Err(invalid("queue progress changed queue cardinality"));
    }
    let differences = current
        .conversation
        .queued_messages
        .iter()
        .zip(&next.conversation.queued_messages)
        .enumerate()
        .filter(|(_, (current, next))| current != next)
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    let [index] = differences.as_slice() else {
        return Err(invalid("queue progress must fence one queue item"));
    };
    let selected = &current.conversation.queued_messages[*index];
    let first_unsettled = current
        .conversation
        .queued_messages
        .iter()
        .position(|message| message.state != QueuedMessageState::Delivered);
    let mut expected = selected.clone();
    expected
        .fence_dispatch(at.into())
        .map_err(|_| invalid("selected queue item cannot be fenced"))?;
    if first_unsettled != Some(*index) || next.conversation.queued_messages[*index] != expected {
        return Err(invalid("queue progress did not fence the FIFO item"));
    }
    Ok((selected.sequence, selected.payload_sha256.clone()))
}

fn validate_queue_completion_set(
    sandbox: &Sandbox,
    agents: &[(Agent, u64)],
    mutation: &SandboxLifecycleMutation,
    touched: &BTreeSet<crate::AgentId>,
) -> Result<(), SandboxStoreError> {
    if mutation.agent_scope == SandboxLifecycleAgentScope::Touched {
        return Err(invalid("queue completion has an invalid agent scope"));
    }
    let expected_touched = live_ids(sandbox, agents);
    for (agent, _) in agents
        .iter()
        .filter(|(agent, _)| sandbox.restores_agent(agent))
    {
        let queue_settled = agent
            .conversation
            .queued_messages
            .iter()
            .all(|message| message.state == QueuedMessageState::Delivered);
        if agent.status != AgentStatus::Ready
            || agent.conversation.active_turn.is_some()
            || blocking_operation(agent)
            || !first_message_is_terminal(agent)
            || agent.work.step.is_pending()
            || agent.work.lease.is_some()
            || agent.requested_stop_reason.is_some()
            || !queue_settled
            || (!agent.conversation.queued_messages.is_empty()
                && !latest_delivery_is_durable(agent))
        {
            return Err(invalid(
                "queue completion requires every live agent to be settled",
            ));
        }
    }
    if *touched != expected_touched {
        return Err(invalid(
            "queue completion did not prune the exact delivered set",
        ));
    }
    Ok(())
}

fn validate_final_publication_agents(
    sandbox: &Sandbox,
    agents: &[(Agent, u64)],
) -> Result<(), SandboxStoreError> {
    if matches!(
        sandbox.lifecycle.operation,
        Some(SandboxLifecycleOperation::Pause {
            phase: SandboxPausePhase::ResumeWriters,
            ..
        })
    ) {
        if agents
            .iter()
            .any(|(agent, _)| sandbox.restores_agent(agent) && agent.status == AgentStatus::Paused)
        {
            return Err(invalid(
                "pause cancellation cannot expose parked agents as active",
            ));
        }
        return Ok(());
    }
    let publishing_active = matches!(
        sandbox.lifecycle.operation,
        Some(SandboxLifecycleOperation::Revive {
            phase: SandboxRevivePhase::PublishActive,
            ..
        }) | Some(SandboxLifecycleOperation::ImageRotation {
            phase: SandboxImageRotationPhase::PublishActive,
            ..
        })
    );
    if !publishing_active {
        return Ok(());
    }
    let settled = agents
        .iter()
        .filter(|(agent, _)| sandbox.restores_agent(agent))
        .all(|(agent, _)| {
            agent.status == AgentStatus::Ready
                && agent.conversation.active_turn.is_none()
                && !blocking_operation(agent)
                && first_message_is_terminal(agent)
                && agent.conversation.queued_messages.is_empty()
                && !agent.work.step.is_pending()
                && agent.work.lease.is_none()
                && agent.requested_stop_reason.is_none()
        });
    if !settled {
        return Err(invalid(
            "active publication requires every live agent to be settled",
        ));
    }
    Ok(())
}

fn conversation_matches_except_generation(current: &Agent, next: &Agent) -> bool {
    current.conversation.conversation_id == next.conversation.conversation_id
        && current.conversation.first_message == next.conversation.first_message
        && current.conversation.active_turn == next.conversation.active_turn
        && current.conversation.operation == next.conversation.operation
        && current.conversation.queued_messages == next.conversation.queued_messages
}

fn blocking_operation(agent: &Agent) -> bool {
    agent
        .conversation
        .operation
        .as_ref()
        .is_some_and(AgentOperationRecord::blocks_dispatch)
}

fn first_message_is_terminal(agent: &Agent) -> bool {
    agent
        .conversation
        .first_message
        .as_ref()
        .is_none_or(|message| message.is_terminal())
}

fn live_ids(sandbox: &Sandbox, agents: &[(Agent, u64)]) -> BTreeSet<crate::AgentId> {
    agents
        .iter()
        .filter(|(agent, _)| sandbox.restores_agent(agent))
        .map(|(agent, _)| agent.id.clone())
        .collect()
}

const fn invalid(detail: &'static str) -> SandboxStoreError {
    SandboxStoreError::InvalidTransaction(detail)
}
