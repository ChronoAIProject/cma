//! Complete persisted child evidence for inactivity without a live Agent.
use super::{Agent, InactivityPauseRefusal as Refusal, Sandbox, Workspace, WorkspaceStatus};

pub fn agentless_idle_with_children(
    sandbox: &Sandbox,
    agents: &[Agent],
    workspaces: &[Workspace],
    observed_at: &str,
    threshold: u64,
) -> bool {
    agentless_idle_refusal(sandbox, agents, workspaces, observed_at, threshold).is_none()
}

pub fn agentless_idle_refusal(
    sandbox: &Sandbox,
    agents: &[Agent],
    workspaces: &[Workspace],
    observed_at: &str,
    threshold: u64,
) -> Option<Refusal> {
    let Some(now) = crate::seconds_from_rfc3339(observed_at) else {
        return Some(Refusal::InvalidActivityTime);
    };
    let time_refusal = |at: &str, recent: Refusal| match crate::seconds_from_rfc3339(at) {
        Some(at) if now > at.saturating_add(threshold) => None,
        Some(_) => Some(recent),
        None => Some(Refusal::InvalidActivityTime),
    };
    for at in [&sandbox.created_at, &sandbox.last_activity_at] {
        if let Some(refusal) = time_refusal(at, Refusal::RecentSandboxActivity) {
            return Some(refusal);
        }
    }
    let Some(runtime) = &sandbox.runtime else {
        return Some(Refusal::InvalidRuntime);
    };
    if let Some(refusal) = time_refusal(&runtime.bound_at, Refusal::RecentRuntimeActivity) {
        return Some(refusal);
    }
    for agent in agents {
        if agent.sandbox_id != sandbox.id || agent.owner != sandbox.owner || agent.status.is_live()
        {
            return Some(Refusal::InvalidAgentSet);
        }
        if agent.work.step.is_pending()
            || agent.work.lease.is_some()
            || agent.conversation.active_turn.is_some()
            || !agent.conversation.queued_messages.is_empty()
        {
            return Some(Refusal::PendingAgentWork);
        }
        if let Some(refusal) = time_refusal(&agent.updated_at, Refusal::RecentAgentActivity) {
            return Some(refusal);
        }
    }
    for workspace in workspaces {
        if workspace.sandbox_id != sandbox.id || workspace.owner != sandbox.owner {
            return Some(Refusal::InvalidStoredEvidence);
        }
        if workspace.work.step.is_pending() || workspace.work.lease.is_some() {
            return Some(Refusal::PendingWorkspaceWork);
        }
        let settled = match workspace.status {
            WorkspaceStatus::Ready => workspace.ready_at.as_deref(),
            WorkspaceStatus::Removed => workspace.removed_at.as_deref(),
            _ => return Some(Refusal::PendingWorkspaceWork),
        };
        let Some(settled) = settled else {
            return Some(Refusal::InvalidActivityTime);
        };
        for at in [workspace.created_at.as_str(), settled] {
            if let Some(refusal) = time_refusal(at, Refusal::RecentWorkspaceActivity) {
                return Some(refusal);
            }
        }
    }
    None
}
