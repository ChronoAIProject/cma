use crate::ids::WorkspaceId;

use super::{Agent, AgentStatus, Sandbox, Workspace, WorkspaceStatus};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SandboxLimitError {
    #[error("workspace limit reached: {live} of {limit}")]
    WorkspaceLimitReached { limit: u8, live: usize },
    #[error("agent limit reached: {live} of {limit}")]
    AgentLimitReached { limit: u8, live: usize },
}

pub fn enforce_workspace_limit(
    sandbox: &Sandbox,
    workspaces: &[Workspace],
) -> Result<(), SandboxLimitError> {
    let live = workspaces
        .iter()
        .filter(|workspace| {
            workspace.sandbox_id == sandbox.id
                && workspace.status != WorkspaceStatus::Removed
                && workspace.removed_at.is_none()
        })
        .count();
    if live >= usize::from(sandbox.limits.max_workspaces) {
        Err(SandboxLimitError::WorkspaceLimitReached {
            limit: sandbox.limits.max_workspaces,
            live,
        })
    } else {
        Ok(())
    }
}

pub fn enforce_agent_limit(
    sandbox: &Sandbox,
    workspace_id: &WorkspaceId,
    agents: &[Agent],
) -> Result<(), SandboxLimitError> {
    let live = agents
        .iter()
        .filter(|agent| {
            agent.sandbox_id == sandbox.id
                && &agent.workspace_id == workspace_id
                && matches!(
                    agent.status,
                    AgentStatus::Starting
                        | AgentStatus::Ready
                        | AgentStatus::Working
                        | AgentStatus::WaitingApproval
                        | AgentStatus::Paused
                )
                && agent.deleted_at.is_none()
        })
        .count();
    if live >= usize::from(sandbox.limits.max_agents_per_workspace) {
        Err(SandboxLimitError::AgentLimitReached {
            limit: sandbox.limits.max_agents_per_workspace,
            live,
        })
    } else {
        Ok(())
    }
}
