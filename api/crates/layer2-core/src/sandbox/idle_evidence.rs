//! Complete persisted child evidence for inactivity without a live Agent.
use super::{Agent, Sandbox, Workspace, WorkspaceStatus};

pub fn agentless_idle_with_children(
    sandbox: &Sandbox,
    agents: &[Agent],
    workspaces: &[Workspace],
    observed_at: &str,
    threshold: u64,
) -> bool {
    let Some(now) = crate::seconds_from_rfc3339(observed_at) else {
        return false;
    };
    let idle = |at: &str| {
        crate::seconds_from_rfc3339(at).is_some_and(|at| now > at.saturating_add(threshold))
    };
    idle(&sandbox.created_at)
        && idle(&sandbox.last_activity_at)
        && sandbox
            .runtime
            .as_ref()
            .is_some_and(|runtime| idle(&runtime.bound_at))
        && agents.iter().all(|agent| {
            agent.sandbox_id == sandbox.id
                && agent.owner == sandbox.owner
                && !agent.status.is_live()
                && !agent.work.step.is_pending()
                && agent.work.lease.is_none()
                && agent.conversation.active_turn.is_none()
                && agent.conversation.queued_messages.is_empty()
                && idle(&agent.updated_at)
        })
        && workspaces.iter().all(|workspace| {
            workspace.sandbox_id == sandbox.id
                && workspace.owner == sandbox.owner
                && !workspace.work.step.is_pending()
                && workspace.work.lease.is_none()
                && matches!(
                    workspace.status,
                    WorkspaceStatus::Ready | WorkspaceStatus::Removed
                )
                && idle(&workspace.created_at)
                && match workspace.status {
                    WorkspaceStatus::Ready => workspace.ready_at.as_deref().is_some_and(idle),
                    WorkspaceStatus::Removed => workspace.removed_at.as_deref().is_some_and(idle),
                    _ => false,
                }
        })
}
