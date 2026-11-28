use super::{changed, FoldedConversationUpdate};
use crate::sandbox::conversation_fold_support::{
    active_turn, append_bounded, flush_message, observe_first_message, observe_operation,
};
use crate::sandbox::{
    Agent, AgentConversationError, AgentLogKind, AgentMessageAssembly,
    MAX_DURABLE_AGENT_MESSAGE_BYTES,
};

pub(super) fn fold_message_chunk(
    current: &Agent,
    assembly: &AgentMessageAssembly,
    chunk: &layer3_adapter::ContentChunk,
    prompt_evidence: bool,
    appended_at: &str,
    incremental_text: bool,
) -> Result<FoldedConversationUpdate, AgentConversationError> {
    active_turn(current)?;
    let layer3_adapter::ContentBlock::Text(text) = &chunk.content else {
        return Ok(FoldedConversationUpdate::Quiet(assembly.clone()));
    };
    let message_id = chunk.message_id.as_ref().map(|id| id.0.to_string());
    let mut agent = current.clone();
    let mut next_assembly = assembly.clone();
    let mut logs = Vec::new();
    if !next_assembly.content.is_empty() && next_assembly.message_id != message_id {
        logs.extend(flush_message(&next_assembly));
        next_assembly = AgentMessageAssembly::default();
    }
    next_assembly.message_id = message_id;
    next_assembly.prompt_evidence =
        (next_assembly.content.is_empty() || next_assembly.prompt_evidence) && prompt_evidence;
    let offset = next_assembly.content.len();
    append_bounded(
        &mut next_assembly.content,
        &text.text,
        MAX_DURABLE_AGENT_MESSAGE_BYTES,
    );
    observe_first_message(&mut agent, prompt_evidence, appended_at, &mut logs)?;
    observe_operation(&mut agent, prompt_evidence, false, appended_at, &mut logs)?;
    if next_assembly.content.len() > offset
        && (next_assembly.text_published || (incremental_text && offset == 0))
    {
        logs.push(AgentLogKind::AgentMessageDelta {
            offset: u32::try_from(offset).expect("message extent is bounded"),
            content: next_assembly.content[offset..].to_owned(),
        });
        next_assembly.text_published = true;
    }
    if logs.is_empty() {
        return Ok(FoldedConversationUpdate::Quiet(next_assembly));
    }
    changed(
        agent,
        logs,
        next_assembly,
        None,
        current.conversation.active_turn,
        false,
        appended_at,
    )
}
