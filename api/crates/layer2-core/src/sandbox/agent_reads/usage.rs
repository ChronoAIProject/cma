use crate::seconds_from_rfc3339;

use super::super::agent_reads_usage::UsageFold;
use super::super::{Agent, AgentLogEntry, AgentLogKind, AgentUsage, AgentUsageReading};

/// Fold only the durable rows that can change usage. This keeps the legacy
/// usage projection exact while allowing a trusted store to omit transcript
/// and tool payloads from the read. The validation mirrors the usage portion
/// of the complete fold; it deliberately does not invent projection state.
pub(super) fn fold_agent_usage(
    agent: &Agent,
    entries: &[AgentLogEntry],
) -> Result<AgentUsageReading, String> {
    agent
        .validate_worktree_recoveries()
        .map_err(|error| error.to_string())?;
    let Some(first) = entries.first() else {
        return Err("agent log has no creation record".into());
    };
    match &first.kind {
        AgentLogKind::Created {
            workspace_id,
            worktree_id,
        } if workspace_id == &agent.workspace_id && worktree_id == agent.creation_worktree_id() => {
        }
        AgentLogKind::Created { .. } => {
            return Err("agent creation log lineage does not match".into());
        }
        _ => return Err("agent log does not begin with creation".into()),
    }

    let mut usage = UsageFold::default();
    let mut latest_conversation = None;
    let mut previous_seq = 0;
    for (index, entry) in entries.iter().enumerate() {
        if entry.agent_id != agent.id || entry.seq == 0 || (index > 0 && entry.seq <= previous_seq)
        {
            return Err("agent usage log identity or sequence is invalid".into());
        }
        if seconds_from_rfc3339(&entry.appended_at).is_none() {
            return Err("agent usage log timestamp is invalid".into());
        }
        previous_seq = entry.seq;
        match &entry.kind {
            AgentLogKind::Created { .. } if entry.seq != 1 => {
                return Err("agent log contains duplicate creation".into());
            }
            AgentLogKind::ConversationOpened {
                conversation_id,
                runtime_generation,
            } => {
                if conversation_id.trim().is_empty()
                    || conversation_id.len() > 512
                    || *runtime_generation == 0
                    || latest_conversation
                        .as_ref()
                        .is_some_and(|(_, previous)| runtime_generation <= previous)
                {
                    return Err("agent conversation boundary is invalid".into());
                }
                latest_conversation = Some((conversation_id.clone(), *runtime_generation));
                usage.open_generation(*runtime_generation)?;
            }
            AgentLogKind::RuntimeGenerationRetired {
                previous_generation,
                runtime_generation,
            } => {
                let Some((_, generation)) = latest_conversation.as_mut() else {
                    return Err("agent runtime retirement has no conversation".into());
                };
                if *generation != *previous_generation
                    || previous_generation.checked_add(1) != Some(*runtime_generation)
                {
                    return Err("agent runtime retirement boundary is invalid".into());
                }
                *generation = *runtime_generation;
            }
            AgentLogKind::UsageUpdated {
                input_tokens,
                output_tokens,
                context_used,
                context_window,
                turn_sequence,
                turn_input_tokens,
                turn_output_tokens,
            } => {
                usage.observed = Some(AgentUsage {
                    input_tokens: *input_tokens,
                    output_tokens: *output_tokens,
                    context_used: *context_used,
                    context_window: *context_window,
                    current_turn: turn_sequence.map(|turn_sequence| super::super::AgentTurnUsage {
                        turn_sequence,
                        input_tokens: *turn_input_tokens,
                        output_tokens: *turn_output_tokens,
                    }),
                });
            }
            AgentLogKind::Usage {
                input_tokens,
                output_tokens,
                context_used,
                context_window,
            } => usage.observe(
                *input_tokens,
                *output_tokens,
                *context_used,
                *context_window,
            )?,
            _ => {}
        }
    }
    match (
        agent.conversation.conversation_id.as_deref(),
        agent.conversation.runtime_generation,
        latest_conversation.as_ref(),
    ) {
        (None, None, None) => {}
        (Some(id), Some(generation), Some((logged_id, logged_generation)))
            if id == logged_id && generation == *logged_generation => {}
        _ => return Err("agent conversation identity disagrees with its log".into()),
    }
    Ok(AgentUsageReading {
        agent_id: agent.id.clone(),
        through_seq: agent.conversation.last_seq,
        usage: usage.reading(),
    })
}
