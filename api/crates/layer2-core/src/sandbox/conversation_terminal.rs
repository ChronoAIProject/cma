use super::*;

pub(super) fn fold_turn_ended(
    current: &Agent,
    assembly: &AgentMessageAssembly,
    completed: layer3_adapter::PromptResponse,
    prompt_evidence: bool,
    appended_at: &str,
) -> Result<FoldedConversationUpdate, AgentConversationError> {
    let turn = active_turn(current)?;
    let mut agent = current.clone();
    let mut logs = flush_message(assembly);
    let interrupted = completed.stop_reason == layer3_adapter::TurnStopReason::Cancelled;
    observe_operation(
        &mut agent,
        prompt_evidence,
        interrupted,
        appended_at,
        &mut logs,
    )?;
    if let Some(usage) = completed
        .usage
        .filter(|_| agent.conversation.observations.usage.is_none())
    {
        logs.push(AgentLogKind::Usage {
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            context_used: 0,
            context_window: None,
        });
    }
    if interrupted {
        fail_first_message(
            &mut agent,
            "the first-message turn was interrupted",
            appended_at,
            &mut logs,
        )?;
        logs.push(AgentLogKind::TurnInterrupted {
            turn_sequence: turn,
        });
    } else {
        complete_first_message(&mut agent, appended_at, &mut logs)?;
        logs.push(AgentLogKind::TurnCompleted {
            turn_sequence: turn,
        });
    }
    logs.push(AgentLogKind::StatusChanged {
        from: current.status,
        to: AgentStatus::Ready,
    });
    changed(
        agent,
        logs,
        AgentMessageAssembly::default(),
        Some(AgentStatus::Ready),
        None,
        true,
        appended_at,
    )
}

pub(super) fn fold_turn_failed(
    current: &Agent,
    assembly: &AgentMessageAssembly,
    message: String,
    capability: Option<String>,
    prompt_evidence: bool,
    appended_at: &str,
) -> Result<FoldedConversationUpdate, AgentConversationError> {
    let turn = active_turn(current)?;
    let mut agent = current.clone();
    let mut logs = flush_message(assembly);
    observe_operation(&mut agent, prompt_evidence, false, appended_at, &mut logs)?;
    fail_first_message(&mut agent, &message, appended_at, &mut logs)?;
    logs.push(AgentLogKind::TurnFailed {
        turn_sequence: turn,
        failure: TypedFailure {
            class: capability.unwrap_or_else(|| "agent_turn_failed".into()),
            detail: bounded(message, 4 * 1024),
        },
    });
    logs.push(AgentLogKind::StatusChanged {
        from: current.status,
        to: AgentStatus::Ready,
    });
    changed(
        agent,
        logs,
        AgentMessageAssembly::default(),
        Some(AgentStatus::Ready),
        None,
        true,
        appended_at,
    )
}
