use layer3_adapter::{ContentBlock, ConversationUpdate};

use super::{
    Agent, AgentConversationError, AgentLogKind, AgentMessageAssembly, FirstMessageDelivery,
    FirstMessageFailure,
};

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
            FirstMessageDelivery::DispatchFenced | FirstMessageDelivery::WriteConfirmed
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
    ) {
        return Ok(());
    }
    let failure = FirstMessageFailure::new(
        "agent_turn_failed",
        bounded(detail.to_owned(), 4 * 1024),
        false,
        at,
    )
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

pub(super) fn prompt_evidence(update: ConversationUpdate) -> (ConversationUpdate, bool) {
    match update {
        ConversationUpdate::WithoutPromptEvidence(update) => {
            let (update, _) = prompt_evidence(*update);
            (update, false)
        }
        update => (update, true),
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
