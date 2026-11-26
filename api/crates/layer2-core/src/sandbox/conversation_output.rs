//! Durable output receipt validation and guarded checkpoint commits.
use super::DurableAgentUpdateRecorder;
use crate::sandbox::store::{
    AgentConversationCapacitySettlement, AgentConversationMutation, AgentMessageCheckpoint,
    AgentReopenGuard, SandboxStoreError,
};
use crate::sandbox::{Agent, AgentConversationError, AgentUpdateOutcome};
use layer3_adapter::{ConversationUpdate, SessionUpdate};
use sha2::{Digest, Sha256};

/// A fresh process loads the retained conversation before the owner can send
/// another turn. Commit those transport receipts without replaying the history
/// into the Agent log. Live turn evidence still requires a published attachment.
pub(super) fn owner_restart_effect(
    update: Option<&ConversationUpdate>,
) -> Result<Option<&ConversationUpdate>, AgentConversationError> {
    match update {
        None | Some(ConversationUpdate::Metadata(_)) => Ok(update),
        Some(ConversationUpdate::Said(
            SessionUpdate::UserMessageChunk(_)
            | SessionUpdate::AgentThoughtChunk(_)
            | SessionUpdate::Plan(_)
            | SessionUpdate::UsageUpdate(_)
            | SessionUpdate::SessionInfoUpdate(_)
            | SessionUpdate::AvailableCommandsUpdate(_)
            | SessionUpdate::ConfigOptionUpdate(_)
            | SessionUpdate::CurrentModeUpdate(_),
        ))
        | Some(ConversationUpdate::Usage(_)) => Ok(None),
        Some(ConversationUpdate::WithoutPromptEvidence(update))
            if matches!(
                update.effect(),
                ConversationUpdate::Said(
                    SessionUpdate::AgentMessageChunk(_)
                        | SessionUpdate::ToolCall(_)
                        | SessionUpdate::ToolCallUpdate(_)
                )
            ) =>
        {
            Ok(None)
        }
        _ => Err(AgentConversationError::ReopenOutputDiscontinuity),
    }
}

pub(super) fn update_parts(
    update: &layer3_adapter::ConversationUpdate,
) -> (
    Option<&layer3_adapter::RuntimeOutputSource>,
    Option<&layer3_adapter::ConversationUpdate>,
    bool,
) {
    use layer3_adapter::ConversationUpdate;
    let retained_effect = matches!(update, ConversationUpdate::RetainedEffect(_));
    match update.effect() {
        ConversationUpdate::RuntimeOutput { receipt, update } => (
            Some(&receipt.source),
            update.as_deref().map(ConversationUpdate::effect),
            update.as_ref().is_some_and(|update| update.is_shared()),
        ),
        update => (None, Some(update), retained_effect),
    }
}

impl DurableAgentUpdateRecorder {
    pub(super) async fn commit_output(
        &self,
        current: Agent,
        previous: Option<AgentMessageCheckpoint>,
        next: AgentMessageCheckpoint,
        mut mutation: Option<AgentConversationMutation>,
        guard: Option<&AgentReopenGuard>,
        identified_terminal: bool,
    ) -> Result<AgentUpdateOutcome, SandboxStoreError> {
        let id = current.id.clone();
        let result = if let Some(guard) = guard {
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
        } else if mutation.is_none() {
            self.store
                .commit_agent_message_update(current, previous, next, None)
                .await
        } else {
            let result = self
                .store
                .commit_agent_message_update(
                    current.clone(),
                    previous.clone(),
                    next.clone(),
                    mutation.clone(),
                )
                .await;
            match (result, mutation) {
                (Err(SandboxStoreError::AgentLogCapacityExceeded), Some(mutation)) => {
                    let settlement = AgentConversationCapacitySettlement::new(
                        current,
                        previous,
                        next,
                        mutation,
                        identified_terminal,
                    )?;
                    self.store.commit_agent_capacity_update(settlement).await
                }
                (result, _) => result,
            }
        };
        if result
            .as_ref()
            .is_ok_and(|outcome| !outcome.appended.is_empty() || outcome.turn_closed)
        {
            self.live.publish_head(&id);
        }
        result
    }
}

pub(super) fn output_replayed(
    source: &layer3_adapter::RuntimeOutputSource,
    current: &Agent,
    previous: Option<&AgentMessageCheckpoint>,
    reopen: bool,
) -> Result<bool, AgentConversationError> {
    // The Agent and checkpoint come from one snapshot. An older generation
    // establishes absence of a replacement cursor only for this conversation.
    let verified = || {
        reopen
            && previous.is_none_or(|checkpoint| {
                checkpoint.validate().is_ok()
                    && checkpoint.native_conversation_id == source.native_conversation_id
                    && current
                        .conversation
                        .runtime_generation
                        .is_some_and(|generation| {
                            (checkpoint.runtime_generation < generation
                                && checkpoint.after_seq <= current.conversation.last_seq)
                                || checkpoint.matches(current)
                        })
            })
    };
    let previous = previous
        .filter(|checkpoint| {
            checkpoint.native_conversation_id == source.native_conversation_id
                && Some(checkpoint.runtime_generation) == current.conversation.runtime_generation
        })
        .and_then(|checkpoint| checkpoint.output.as_ref());
    output_position_replayed(source, current, previous, verified)
}

pub(super) fn output_position_replayed(
    source: &layer3_adapter::RuntimeOutputSource,
    current: &Agent,
    previous: Option<&layer3_adapter::RuntimeOutputSource>,
    verified: impl FnOnce() -> bool,
) -> Result<bool, AgentConversationError> {
    let invalid = || {
        AgentConversationError::Corrupt(
            "runtime output receipt is not contiguous with its conversation".into(),
        )
    };
    if current.conversation.conversation_id.as_deref() != Some(&source.native_conversation_id)
        || uuid::Uuid::parse_str(&source.stream_id).is_err()
        || source.native_conversation_id.len() > 4096
        || source.cursor == 0
        || source.payload_sha256.len() != 64
        || !source
            .payload_sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(invalid());
    }
    let discontinuity = || {
        if verified() && !source.native_conversation_id.is_empty() {
            AgentConversationError::ReopenOutputDiscontinuity
        } else {
            invalid()
        }
    };
    match previous {
        Some(previous) if previous.stream_id != source.stream_id => Err(discontinuity()),
        Some(previous)
            if source.cursor == previous.cursor
                && source.payload_sha256 != previous.payload_sha256 =>
        {
            Err(discontinuity())
        }
        Some(previous) if source.cursor <= previous.cursor => Ok(true),
        Some(previous) if previous.cursor.checked_add(1) == Some(source.cursor) => Ok(false),
        None if source.cursor == 1 => Ok(false),
        _ => Err(discontinuity()),
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

pub(super) fn store_error(error: SandboxStoreError) -> AgentConversationError {
    match error {
        SandboxStoreError::NotFound => AgentConversationError::NotFound,
        SandboxStoreError::InvalidTransaction(detail) => {
            AgentConversationError::InvalidState(detail)
        }
        SandboxStoreError::AgentLogCapacityExceeded => AgentConversationError::CapacityExceeded,
        SandboxStoreError::Corrupt(detail) => AgentConversationError::Corrupt(detail),
        other => AgentConversationError::Unavailable(other.to_string()),
    }
}

pub(super) fn fingerprint(
    source: Option<&layer3_adapter::RuntimeOutputSource>,
    update: Option<&layer3_adapter::ConversationUpdate>,
) -> String {
    source.map_or_else(
        || {
            format!(
                "{:x}",
                Sha256::digest(
                    format!("{:?}", update.as_ref().expect("legacy update is present")).as_bytes()
                )
            )
        },
        |source| format!("{:x}", Sha256::digest(format!("{source:?}").as_bytes())),
    )
}
