use layer3_adapter::{ContentBlock, ConversationUpdate};

use super::{
    Agent, AgentConversationError, AgentLogKind, AgentMessageAssembly, AgentOperationDelivery,
    AgentOperationIntent, FirstMessageDelivery, FirstMessageFailure,
};

pub(super) fn observe_operation(
    agent: &mut Agent,
    prompt_evidence: bool,
    interruption_evidence: bool,
    at: &str,
    logs: &mut Vec<AgentLogKind>,
) -> Result<(), AgentConversationError> {
    let Some(record) = agent.conversation.operation.as_ref() else {
        return Ok(());
    };
    let evidence_matches = match &record.intent {
        AgentOperationIntent::Interrupt => interruption_evidence,
        AgentOperationIntent::UpdateSettings { .. }
        | AgentOperationIntent::RefreshContext { .. } => false,
        _ => prompt_evidence,
    };
    if !evidence_matches
        || !matches!(
            record.state,
            AgentOperationDelivery::DispatchFenced
                | AgentOperationDelivery::WriteConfirmed
                | AgentOperationDelivery::RecoveryRequired
        )
    {
        return Ok(());
    }
    let queue_index = matching_queued_dispatch(agent)?;
    let record = agent
        .conversation
        .operation
        .as_mut()
        .expect("the observed operation was checked");
    record
        .mark_execution_observed(at.to_owned())
        .map_err(|_| AgentConversationError::InvalidState("operation evidence was refused"))?;
    logs.push(AgentLogKind::OperationDeliveryChanged {
        operation_sequence: record.sequence,
        operation: record.intent.kind(),
        state: record.state,
    });
    if let Some(index) = queue_index {
        agent.conversation.queued_messages[index]
            .mark_execution_observed()
            .map_err(|_| {
                AgentConversationError::InvalidState("queued-message evidence was refused")
            })?;
    }
    Ok(())
}

fn matching_queued_dispatch(agent: &Agent) -> Result<Option<usize>, AgentConversationError> {
    let Some(operation) = agent.conversation.operation.as_ref() else {
        return Ok(None);
    };
    let AgentOperationIntent::SendMessage { payload_sha256 } = &operation.intent else {
        return Ok(None);
    };
    let matches = agent
        .conversation
        .queued_messages
        .iter()
        .enumerate()
        .filter(|(_, message)| {
            matches!(
                message.state,
                super::QueuedMessageState::DispatchFenced
                    | super::QueuedMessageState::RecoveryRequired
            ) && message.payload_sha256 == *payload_sha256
                && message.dispatch_fenced_at.as_deref()
                    == Some(operation.dispatch_fenced_at.as_str())
        })
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [] => Ok(None),
        [index] => Ok(Some(*index)),
        _ => Err(AgentConversationError::Corrupt(
            "multiple queue items match one operation fence".into(),
        )),
    }
}

pub(super) fn observe_first_message(
    agent: &mut Agent,
    prompt_evidence: bool,
    at: &str,
    logs: &mut Vec<AgentLogKind>,
) -> Result<(), AgentConversationError> {
    let Some(record) = agent.conversation.first_message.as_mut() else {
        return Ok(());
    };
    if !prompt_evidence
        || !matches!(
            record.state,
            FirstMessageDelivery::DispatchFenced
                | FirstMessageDelivery::WriteConfirmed
                | FirstMessageDelivery::RecoveryRequired
        )
    {
        return Ok(());
    }
    record
        .mark_execution_observed(at.to_owned())
        .map_err(|_| AgentConversationError::InvalidState("first-message evidence was refused"))?;
    logs.push(AgentLogKind::FirstMessageChanged {
        state: record.state,
        recovery_generation: record.recovery_generation,
    });
    Ok(())
}

pub(super) fn complete_first_message(
    agent: &mut Agent,
    at: &str,
    logs: &mut Vec<AgentLogKind>,
) -> Result<(), AgentConversationError> {
    let Some(record) = agent.conversation.first_message.as_mut() else {
        return Ok(());
    };
    if !matches!(
        record.state,
        FirstMessageDelivery::DispatchFenced
            | FirstMessageDelivery::WriteConfirmed
            | FirstMessageDelivery::ExecutionObserved
            | FirstMessageDelivery::RecoveryRequired
    ) {
        return Ok(());
    }
    record.complete(at.to_owned()).map_err(|_| {
        AgentConversationError::InvalidState("first-message completion was refused")
    })?;
    logs.push(AgentLogKind::FirstMessageChanged {
        state: record.state,
        recovery_generation: record.recovery_generation,
    });
    Ok(())
}

pub(super) fn fail_first_message(
    agent: &mut Agent,
    class: &str,
    detail: &str,
    at: &str,
    logs: &mut Vec<AgentLogKind>,
) -> Result<(), AgentConversationError> {
    let Some(record) = agent.conversation.first_message.as_mut() else {
        return Ok(());
    };
    if !matches!(
        record.state,
        FirstMessageDelivery::DispatchFenced
            | FirstMessageDelivery::WriteConfirmed
            | FirstMessageDelivery::ExecutionObserved
            | FirstMessageDelivery::RecoveryRequired
    ) {
        return Ok(());
    }
    let class = if class.trim().is_empty() || class.len() > 128 {
        "agent_turn_failed"
    } else {
        class
    };
    let failure = FirstMessageFailure::new(class, bounded(detail.to_owned(), 4 * 1024), false, at)
        .map_err(|_| AgentConversationError::InvalidState("first-message failure was invalid"))?;
    record
        .fail(failure)
        .map_err(|_| AgentConversationError::InvalidState("first-message failure was refused"))?;
    logs.push(AgentLogKind::FirstMessageChanged {
        state: record.state,
        recovery_generation: record.recovery_generation,
    });
    Ok(())
}

pub(super) fn flush_message(assembly: &AgentMessageAssembly) -> Vec<AgentLogKind> {
    (!assembly.content.is_empty())
        .then(|| AgentLogKind::AgentMessage {
            content: assembly.content.clone(),
        })
        .into_iter()
        .collect()
}

pub(super) fn active_turn(agent: &Agent) -> Result<u64, AgentConversationError> {
    agent
        .conversation
        .active_turn
        .ok_or(AgentConversationError::InvalidState("no turn is active"))
}

pub(super) fn prompt_evidence(update: &ConversationUpdate) -> (&ConversationUpdate, bool) {
    match update.effect() {
        ConversationUpdate::WithoutPromptEvidence(update) => {
            let (update, _) = prompt_evidence(update);
            (update, false)
        }
        update => {
            let is_evidence = update.turn_evidence().is_some();
            (update, is_evidence)
        }
    }
}

pub(super) fn approval_detail(options: &[layer3_adapter::PermissionOption]) -> Option<String> {
    let names = options
        .iter()
        .map(|option| option.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    (!names.is_empty()).then(|| bounded(names, 4 * 1024))
}

pub(super) fn plain_text(content: &ContentBlock) -> Option<String> {
    match content {
        ContentBlock::Text(text) => Some(text.text.clone()),
        _ => None,
    }
}

pub(super) fn validate_prompt_identity(
    current: &Agent,
    prompt_id: &layer3_adapter::PromptIdentity,
) -> Result<(), AgentConversationError> {
    let conversation = current.conversation.conversation_id.as_deref().ok_or(
        AgentConversationError::InvalidState("terminal prompt identity has no conversation"),
    )?;
    let generation =
        current
            .conversation
            .runtime_generation
            .ok_or(AgentConversationError::InvalidState(
                "terminal prompt identity has no generation",
            ))?;
    let turn = current
        .conversation
        .active_turn
        .ok_or(AgentConversationError::InvalidState(
            "terminal prompt identity has no active turn",
        ))?;
    if layer3_adapter::PromptIdentity::new(conversation, generation, turn) != *prompt_id {
        return Err(AgentConversationError::InvalidState(
            "terminal prompt identity does not match the active turn",
        ));
    }
    Ok(())
}

pub(super) fn wire_name<T: serde::Serialize>(value: &T) -> String {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(name)) => name,
        _ => "unknown".into(),
    }
}

pub(super) fn append_bounded(target: &mut String, value: &str, limit: usize) {
    if target.len() >= limit {
        return;
    }
    let remaining = limit - target.len();
    if value.len() <= remaining {
        target.push_str(value);
        return;
    }
    let mut end = remaining;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    target.push_str(&value[..end]);
}

pub(super) fn bounded_ref(value: &str, limit: usize) -> String {
    let mut end = value.len().min(limit);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}

pub(super) fn bounded(mut value: String, limit: usize) -> String {
    if value.len() <= limit {
        return value;
    }
    let mut end = limit;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    value.truncate(end);
    value
}

pub(super) fn recorded_tool_output(
    fields: &layer3_adapter::ToolCallUpdateFields,
) -> Option<super::RecordedToolOutput> {
    recorded_tool_blocks(layer3_adapter::reported_tool_text(fields))
}

pub(super) fn recorded_initial_tool_output(
    call: &layer3_adapter::ToolCall,
) -> Option<super::RecordedToolOutput> {
    recorded_tool_blocks(layer3_adapter::reported_tool_call_text(call))
}

fn recorded_tool_blocks<'a>(
    blocks: impl IntoIterator<Item = std::borrow::Cow<'a, str>>,
) -> Option<super::RecordedToolOutput> {
    let mut text = String::new();
    let mut reported = false;
    let mut truncated = false;
    for block in blocks {
        if reported {
            truncated |= text.len() == 16 * 1024;
            append_bounded(&mut text, "\n", 16 * 1024);
        }
        reported = true;
        truncated |= text.len().saturating_add(block.len()) > 16 * 1024;
        append_bounded(&mut text, block.as_ref(), 16 * 1024);
    }
    reported.then_some(super::RecordedToolOutput { text, truncated })
}

pub(super) fn settle_turn_policy(
    agent: &mut Agent,
    logs: &mut Vec<AgentLogKind>,
) -> Result<(), AgentConversationError> {
    if let Some(timeout) = agent
        .conversation
        .turn_timeout
        .as_ref()
        .filter(|timeout| timeout.is_open())
    {
        let superseded = timeout.supersede(timeout.revision()).map_err(|_| {
            AgentConversationError::InvalidState("turn deadline settlement was refused")
        })?;
        let resolved = superseded
            .approval_events_since(timeout)
            .map_err(|_| AgentConversationError::InvalidState("approval resolution was refused"))?;
        let position = logs
            .iter()
            .position(|kind| {
                matches!(
                    kind,
                    AgentLogKind::TurnCompleted { .. }
                        | AgentLogKind::TurnFailed { .. }
                        | AgentLogKind::TurnInterrupted { .. }
                )
            })
            .unwrap_or(logs.len());
        logs.splice(position..position, resolved);
        agent.conversation.turn_timeout = Some(superseded);
    }
    Ok(())
}
