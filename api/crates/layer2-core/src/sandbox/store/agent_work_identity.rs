use crate::sandbox::Agent;

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
