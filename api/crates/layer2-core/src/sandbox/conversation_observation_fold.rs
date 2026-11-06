use super::conversation_fold::{changed, FoldedConversationUpdate};
use super::{
    Agent, AgentConversationError, AgentLogKind, AgentMessageAssembly, AgentTurnUsage,
    AgentUsageCheckpoint,
};

pub(super) fn metadata(
    current: &Agent,
    assembly: &AgentMessageAssembly,
    observation: layer3_adapter::ConversationMetadata,
    at: &str,
) -> Result<FoldedConversationUpdate, AgentConversationError> {
    let mut agent = current.clone();
    let metadata = &mut agent.conversation.observations.metadata;
    if let Some(path) = observation
        .working_directory
        .filter(|p| p.starts_with('/') && p.len() <= 4096 && !p.chars().any(char::is_control))
    {
        metadata.working_directory = Some(path);
    }
    if let Some(description) = observation
        .description
        .filter(|d| !d.trim().is_empty() && d.len() <= 4096 && !d.chars().any(char::is_control))
    {
        let has_task_title = metadata.description_source
            == Some(super::AgentDescriptionSource::ProviderTitle)
            || (metadata.description_source == Some(super::AgentDescriptionSource::PromptPreview)
                && !metadata.description_is_setup_preview);
        if !has_task_title
            || observation.description_source == Some(super::AgentDescriptionSource::ProviderTitle)
        {
            let retained_setup = observation.description_input.is_none()
                && metadata.description.as_ref() == Some(&description)
                && metadata.description_is_setup_preview;
            metadata.description_is_setup_preview = observation.description_source
                == Some(super::AgentDescriptionSource::PromptPreview)
                && (retained_setup
                    || current
                        .conversation
                        .first_message
                        .as_ref()
                        .is_some_and(|message| {
                            message.purpose == Some(super::FirstMessagePurpose::Setup)
                                && message.turn_sequence.is_some()
                                && match observation.description_input.as_ref() {
                                    Some(
                                        layer3_adapter::ConversationDescriptionInput::Current(text),
                                    ) => {
                                        message.turn_sequence == current.conversation.active_turn
                                            && text == &message.text
                                    }
                                    Some(layer3_adapter::ConversationDescriptionInput::First(
                                        text,
                                    )) => text == &message.text,
                                    None => false,
                                }
                        }));
            metadata.description = Some(description);
            metadata.description_source = observation.description_source;
        }
    }
    if *metadata == current.conversation.observations.metadata {
        return Ok(FoldedConversationUpdate::Quiet(Box::new(assembly.clone())));
    }
    let metadata = metadata.clone();
    changed(
        agent,
        vec![AgentLogKind::RuntimeMetadataChanged { metadata }],
        assembly.clone(),
        None,
        current.conversation.active_turn,
        false,
        at,
    )
}

pub(super) fn usage(
    current: &Agent,
    assembly: &AgentMessageAssembly,
    observation: layer3_adapter::ConversationUsage,
    at: &str,
) -> Result<FoldedConversationUpdate, AgentConversationError> {
    let Some(conversation_id) = current.conversation.conversation_id.as_ref() else {
        return Ok(FoldedConversationUpdate::Quiet(Box::new(assembly.clone())));
    };
    if observation.turn_id.is_empty()
        || observation.turn_id.len() > 256
        || observation.turn_id.chars().any(char::is_control)
    {
        return Ok(FoldedConversationUpdate::Quiet(Box::new(assembly.clone())));
    }
    let previous = current.conversation.observations.usage.as_ref();
    let mut next = previous.cloned().unwrap_or_default();
    let same = previous.is_some_and(|p| p.conversation_id == *conversation_id);
    // Explicit consumed-counter resets carry no consumed tokens themselves.
    // Context usage may fall at compaction; lifetime usage never does.
    let delta = if observation.counters_reset
        && observation.input_tokens == 0
        && observation.output_tokens == 0
    {
        (0, 0)
    } else if same {
        if observation.input_tokens < next.source_input
            || observation.output_tokens < next.source_output
        {
            // A regressing snapshot without an explicit reset is ambiguous. Do
            // not turn replay, estimates, or missing evidence into consumption.
            return Ok(FoldedConversationUpdate::Quiet(Box::new(assembly.clone())));
        }
        (
            observation.input_tokens - next.source_input,
            observation.output_tokens - next.source_output,
        )
    } else {
        (observation.input_tokens, observation.output_tokens)
    };
    next.input_tokens = next.input_tokens.saturating_add(delta.0);
    next.output_tokens = next.output_tokens.saturating_add(delta.1);
    if let Some(turn_sequence) = current.conversation.active_turn {
        let newly_seen_turn = next
            .current_turn
            .as_ref()
            .is_none_or(|t| t.turn_sequence != turn_sequence);
        let turn = next.current_turn.get_or_insert(AgentTurnUsage {
            turn_sequence,
            input_tokens: 0,
            output_tokens: 0,
        });
        if newly_seen_turn {
            *turn = AgentTurnUsage {
                turn_sequence,
                input_tokens: 0,
                output_tokens: 0,
            };
        }
        let turn_delta = if previous.is_none() {
            (
                observation.last_input_tokens,
                observation.last_output_tokens,
            )
        } else {
            delta
        };
        // A resume snapshot for the previous source turn can update the lifetime
        // baseline without attributing historical usage to a newly admitted turn.
        if !newly_seen_turn || previous.is_none() || next.source_turn != observation.turn_id {
            turn.input_tokens = turn.input_tokens.saturating_add(turn_delta.0);
            turn.output_tokens = turn.output_tokens.saturating_add(turn_delta.1);
        }
    }
    next.conversation_id = conversation_id.clone();
    next.source_turn = observation.turn_id;
    next.source_input = observation.input_tokens;
    next.source_output = observation.output_tokens;
    next.context_used = observation.context_used;
    next.context_window = observation.context_window;
    if previous == Some(&next) {
        return Ok(FoldedConversationUpdate::Quiet(Box::new(assembly.clone())));
    }
    let log = usage_log(&next);
    let mut agent = current.clone();
    agent.conversation.observations.usage = Some(next);
    changed(
        agent,
        vec![log],
        assembly.clone(),
        None,
        current.conversation.active_turn,
        false,
        at,
    )
}

fn usage_log(usage: &AgentUsageCheckpoint) -> AgentLogKind {
    AgentLogKind::UsageUpdated {
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        context_used: usage.context_used,
        context_window: usage.context_window,
        turn_sequence: usage.current_turn.map(|t| t.turn_sequence),
        turn_input_tokens: usage.current_turn.map_or(0, |t| t.input_tokens),
        turn_output_tokens: usage.current_turn.map_or(0, |t| t.output_tokens),
    }
}
