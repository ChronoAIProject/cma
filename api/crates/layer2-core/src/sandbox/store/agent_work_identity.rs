use super::agent_work_transition_validation::PublicationKind;
use super::{AgentWorkMutation, SandboxStoreError};
use crate::sandbox::{Agent, AgentWorkStep};

pub(super) fn immutable_agent_fields_changed(current: &Agent, next: &Agent) -> bool {
    current.work.worktree_recoveries != next.work.worktree_recoveries
        || current.id != next.id
        || current.worktree_id != next.worktree_id
        || current.workspace_id != next.workspace_id
        || current.sandbox_id != next.sandbox_id
        || current.owner != next.owner
        || current.creation_key != next.creation_key
        || current.creation_fingerprint != next.creation_fingerprint
        || current.profile != next.profile
        || current.conversation.managed_guidance_revision
            != next.conversation.managed_guidance_revision
        || current.conversation.preferences != next.conversation.preferences
        || current.conversation.settings != next.conversation.settings
        || current.conversation.turn_timeout != next.conversation.turn_timeout
        || current.capabilities != next.capabilities
        || current.title != next.title
        || current.created_at != next.created_at
        || current.origin != next.origin
}

pub(super) fn first_message_matches(
    current: &Agent,
    next: &Agent,
    mutation: &super::AgentWorkMutation,
    kind: super::agent_work_transition_validation::PublicationKind,
) -> Result<bool, super::SandboxStoreError> {
    use super::agent_work_transition_validation::PublicationKind;
    use crate::sandbox::AgentWorkStep;
    let mut expected = current.conversation.first_message.clone();
    if kind == PublicationKind::StartupFailure
        && matches!(
            current.work.step,
            AgentWorkStep::OpenConversation | AgentWorkStep::PublishReady
        )
    {
        if let Some(first) = expected.as_mut() {
            first
                .fail_startup(mutation.appended_at.clone())
                .map_err(|_| super::SandboxStoreError::Conflict)?;
        }
    }
    Ok(expected == next.conversation.first_message)
}

pub(super) fn validate_agent_delta(
    current: &Agent,
    next: &Agent,
    mutation: &AgentWorkMutation,
    kind: PublicationKind,
) -> Result<(), SandboxStoreError> {
    let identity_changed = current.conversation.conversation_id
        != next.conversation.conversation_id
        || current.conversation.runtime_generation != next.conversation.runtime_generation;
    let opened = kind == PublicationKind::Advance
        && current.work.step == AgentWorkStep::OpenConversation
        && current.conversation.conversation_id.is_none()
        && current.conversation.runtime_generation.is_none()
        && next.conversation.conversation_id.is_some()
        && next.conversation.runtime_generation == Some(1);
    let interaction_matches = match mutation.agent_logs.first() {
        Some(crate::sandbox::AgentLogKind::TurnInterrupted { .. })
            if kind == PublicationKind::Stop =>
        {
            let mut expected = current.clone();
            expected.settle_owner_stop_turn().as_ref() == mutation.agent_logs.first()
                && expected.conversation.active_turn == next.conversation.active_turn
                && expected.conversation.observations == next.conversation.observations
        }
        _ => current.conversation.active_turn == next.conversation.active_turn,
    };
    if !interaction_matches
        || !first_message_matches(current, next, mutation, kind)?
        || current.conversation.operation != next.conversation.operation
        || current.conversation.queued_messages != next.conversation.queued_messages
        || (identity_changed && !opened)
    {
        return Err(SandboxStoreError::Conflict);
    }
    let unchanged = current.status == next.status
        && current.failure == next.failure
        && current.ready_at == next.ready_at
        && current.stopped_at == next.stopped_at
        && current.requested_stop_reason == next.requested_stop_reason
        && current.stop_reason == next.stop_reason
        && current.deleted_at == next.deleted_at;
    let exact = match kind {
        PublicationKind::Intent
        | PublicationKind::StartupNotice
        | PublicationKind::Retry
        | PublicationKind::Advance => unchanged,
        PublicationKind::Ready => {
            current.failure == next.failure
                && next.ready_at.as_deref() == Some(&mutation.appended_at)
                && current.stopped_at == next.stopped_at
                && current.requested_stop_reason == next.requested_stop_reason
                && current.stop_reason == next.stop_reason
                && current.deleted_at == next.deleted_at
        }
        PublicationKind::StartupFailure => {
            current.failure.is_none()
                && next.failure.is_some()
                && current.ready_at == next.ready_at
                && current.stopped_at == next.stopped_at
                && current.requested_stop_reason == next.requested_stop_reason
                && current.stop_reason == next.stop_reason
                && current.deleted_at == next.deleted_at
        }
        PublicationKind::Stop => {
            current.failure == next.failure
                && current.ready_at == next.ready_at
                && next.stopped_at.as_deref() == Some(&mutation.appended_at)
                && next.requested_stop_reason.is_none()
                && next.stop_reason.as_ref() == current.requested_stop_reason.as_ref()
                && current.deleted_at == next.deleted_at
        }
        PublicationKind::Delete => {
            current.failure == next.failure
                && current.ready_at == next.ready_at
                && current.stopped_at == next.stopped_at
                && current.requested_stop_reason == next.requested_stop_reason
                && current.stop_reason == next.stop_reason
                && next.deleted_at.as_deref() == Some(&mutation.appended_at)
        }
    };
    if exact {
        Ok(())
    } else {
        Err(SandboxStoreError::Conflict)
    }
}
