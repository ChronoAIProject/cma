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
