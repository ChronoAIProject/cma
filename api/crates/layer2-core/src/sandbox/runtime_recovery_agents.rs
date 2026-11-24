use super::{
    Agent, AgentLogKind, AgentOperationFailure, AgentStatus, AgentStopReason, AgentWorkStep,
    FirstMessageDelivery, QueuedMessageState, Sandbox, SandboxModelError, SandboxStatus,
};

/// Derive one recovery mutation without replacing any durable conversation log.
pub(super) fn recover_agent(
    sandbox: &Sandbox,
    current: &Agent,
    at: &str,
    native_retirement_finality: bool,
) -> Result<(Agent, Vec<AgentLogKind>), SandboxModelError> {
    current.validate_conversation()?;
    let retiring = sandbox.status == SandboxStatus::Retiring;
    let retained_home = !retiring
        && sandbox
            .lifecycle
            .home
            .as_ref()
            .is_some_and(|home| home.attachment().is_some())
        && sandbox.credential.current.is_some();
    // Preserve unfinished creation and explicit cleanup. The new runtime generation
    // invalidates process receipts, while filesystem intents remain authoritative.
    if retained_home
        && (current.status == AgentStatus::Starting || current.requested_stop_reason.is_some())
    {
        let mut next = current.clone();
        next.work.lease = None;
        next.work.ready_at = at.into();
        let mut logs = Vec::new();
        if let Some(previous_generation) = current.conversation.runtime_generation {
            let runtime_generation = previous_generation
                .checked_add(1)
                .ok_or_else(invalid_sequence)?;
            next.conversation.runtime_generation = Some(runtime_generation);
            logs.push(AgentLogKind::RuntimeGenerationRetired {
                previous_generation,
                runtime_generation,
            });
        }
        // PublishReady retains its exact open receipt and conversation ID. Its
        // normal reconnect path loads that conversation before publishing Ready.
        next.conversation.last_seq = current
            .conversation
            .last_seq
            .checked_add(u64::try_from(logs.len()).map_err(|_| invalid_sequence())?)
            .ok_or_else(invalid_sequence)?;
        next.updated_at = at.into();
        next.revision = current
            .revision
            .checked_add(1)
            .ok_or_else(invalid_sequence)?;
        next.validate_conversation()?;
        return Ok((next, logs));
    }
    let covered = !retiring
        && current.status != AgentStatus::Starting
        && current.requested_stop_reason.is_none()
        && (retained_home && current.conversation.conversation_id.is_some()
            || sandbox
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
                }));
    let mut next = current.clone();
    let mut logs = Vec::new();
    logs.extend(next.conversation.interrupt_active_turn());
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
    if retiring {
        if !native_retirement_finality {
            next.conversation.queued_messages.clear();
        }
        if let Some(previous_generation) = current.conversation.runtime_generation {
            let runtime_generation = previous_generation
                .checked_add(1)
                .ok_or_else(invalid_sequence)?;
            next.conversation.runtime_generation = Some(runtime_generation);
            logs.push(AgentLogKind::RuntimeGenerationRetired {
                previous_generation,
                runtime_generation,
            });
        }
    }
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
        let reason = next.requested_stop_reason.clone().unwrap_or(if retiring {
            AgentStopReason::SandboxRetired
        } else {
            AgentStopReason::RuntimeLost
        });
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
    if !covered {
        if let Some(skill) = next
            .work
            .live_skills
            .last_mut()
            .filter(|s| s.status == super::AgentSkillInstallStatus::Pending)
        {
            skill.status = super::AgentSkillInstallStatus::Failed;
            skill.error = Some("runtime_lost_before_skill_confirmation".into());
        }
    }
    next.work.schedule(AgentWorkStep::None, at.into());
    if retained_home {
        next.work.receipts.retain(|receipt| {
            !matches!(
                receipt.step,
                AgentWorkStep::OpenConversation
                    | AgentWorkStep::PublishReady
                    | AgentWorkStep::PublishConnection
            )
        });
        next.work
            .cleanup_debt
            .retain(|debt| debt.step != AgentWorkStep::OpenConversation);
    } else {
        next.work.receipts.clear();
        next.work.cleanup_debt.clear();
    }
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
