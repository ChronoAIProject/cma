//! Durable output receipt validation and guarded checkpoint commits.
use super::DurableAgentUpdateRecorder;
use crate::sandbox::store::{
    AgentConversationMutation, AgentMessageCheckpoint, AgentReopenGuard, SandboxStoreError,
};
use crate::sandbox::{Agent, AgentConversationError, AgentUpdateOutcome};

impl DurableAgentUpdateRecorder {
    pub(super) async fn commit_output(
        &self,
        current: Agent,
        previous: Option<AgentMessageCheckpoint>,
        next: AgentMessageCheckpoint,
        mut mutation: Option<AgentConversationMutation>,
        guard: Option<&AgentReopenGuard>,
    ) -> Result<AgentUpdateOutcome, SandboxStoreError> {
        if let Some(guard) = guard {
            if let Some(mutation) = &mut mutation {
                mutation.runtime_generation = current
                    .conversation
                    .runtime_generation
                    .ok_or(SandboxStoreError::Conflict)?;
                mutation.agent.conversation.runtime_generation =
                    current.conversation.runtime_generation;
            }
            self.store
                .commit_agent_reopen_update(current, previous, next, mutation, guard.clone())
                .await
        } else {
            self.store
                .commit_agent_message_update(current, previous, next, mutation)
                .await
        }
    }
}

pub(super) fn output_replayed(
    source: &layer3_adapter::RuntimeOutputSource,
    current: &Agent,
    previous: Option<&AgentMessageCheckpoint>,
) -> Result<bool, AgentConversationError> {
    let invalid = || {
        AgentConversationError::Corrupt(
            "runtime output receipt is not contiguous with its conversation".into(),
        )
    };
    if current.conversation.conversation_id.as_deref() != Some(&source.native_conversation_id)
        || uuid::Uuid::parse_str(&source.stream_id).is_err()
        || source.cursor == 0
        || source.payload_sha256.len() != 64
        || !source
            .payload_sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(invalid());
    }
    let previous = previous
        .filter(|checkpoint| {
            checkpoint.native_conversation_id == source.native_conversation_id
                && Some(checkpoint.runtime_generation) == current.conversation.runtime_generation
        })
        .and_then(|checkpoint| checkpoint.output.as_ref());
    match previous {
        Some(previous) if previous.stream_id != source.stream_id => Err(invalid()),
        Some(previous)
            if source.cursor == previous.cursor
                && source.payload_sha256 != previous.payload_sha256 =>
        {
            Err(invalid())
        }
        Some(previous) if source.cursor <= previous.cursor => Ok(true),
        Some(previous) if previous.cursor.checked_add(1) == Some(source.cursor) => Ok(false),
        None if source.cursor == 1 => Ok(false),
        _ => Err(invalid()),
    }
}

pub(super) fn is_identity_terminal_for_current(
    current: &Agent,
    update: Option<&layer3_adapter::ConversationUpdate>,
) -> bool {
    let Some(update) = update else { return false };
    let update = match update {
        layer3_adapter::ConversationUpdate::WithoutPromptEvidence(update) => update.as_ref(),
        update => update,
    };
    let prompt_id = match update {
        layer3_adapter::ConversationUpdate::TurnEndedForPrompt { prompt_id, .. }
        | layer3_adapter::ConversationUpdate::FailedForPrompt { prompt_id, .. } => prompt_id,
        _ => return false,
    };
    let (Some(conversation), Some(generation), Some(turn)) = (
        current.conversation.conversation_id.as_deref(),
        current.conversation.runtime_generation,
        current.conversation.active_turn,
    ) else {
        return false;
    };
    layer3_adapter::PromptIdentity::new(conversation, generation, turn) == *prompt_id
}

pub(super) fn output_source_for_checkpoint(
    source: Option<&layer3_adapter::RuntimeOutputSource>,
    previous: Option<&AgentMessageCheckpoint>,
    runtime_generation: u64,
    identity_terminal: bool,
) -> Option<layer3_adapter::RuntimeOutputSource> {
    let previous = previous
        .filter(|checkpoint| checkpoint.runtime_generation == runtime_generation)
        .and_then(|checkpoint| checkpoint.output.as_ref());
    match (source, previous) {
        (Some(source), Some(previous)) if identity_terminal && source.cursor < previous.cursor => {
            Some(previous.clone())
        }
        (Some(source), _) => Some(source.clone()),
        (None, Some(previous)) => Some(previous.clone()),
        (None, None) => None,
    }
}
