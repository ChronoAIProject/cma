use super::{AgentWorkMutation, SandboxMutationGuard, SandboxStoreError};
use crate::sandbox::{
    Agent, AgentSkillInstallStatus, AgentStatus, AgentWorkStep, Sandbox, SandboxHome,
    SandboxStatus, Workspace, WorkspaceStatus, Worktree, WorktreeStatus,
};
use crate::AgentId;

/// Transient authority captured before obtaining a skill result. Stores must
/// validate it and write the parents in the publication's own transaction.
#[derive(Debug, Clone)]
pub struct AgentSkillResultGuard {
    agent_id: AgentId,
    runtime_generation: Option<u64>,
    sandbox: SandboxMutationGuard,
    home: Option<Box<SandboxHome>>,
    workspace: Workspace,
    worktree: Worktree,
}

impl AgentSkillResultGuard {
    pub fn capture(
        agent: &Agent,
        sandbox: &Sandbox,
        workspace: &Workspace,
        worktree: &Worktree,
    ) -> Result<Self, SandboxStoreError> {
        eligible(agent, sandbox, workspace, worktree)?;
        Ok(Self {
            agent_id: agent.id.clone(),
            runtime_generation: agent.conversation.runtime_generation,
            sandbox: SandboxMutationGuard::capture(sandbox),
            home: sandbox.lifecycle.home.clone(),
            workspace: workspace.clone(),
            worktree: worktree.clone(),
        })
    }

    pub fn validate(
        &self,
        current: &Agent,
        sandbox: &Sandbox,
        workspace: &Workspace,
        worktree: &Worktree,
    ) -> Result<(), SandboxStoreError> {
        eligible(current, sandbox, workspace, worktree)?;
        if current.id != self.agent_id
            || current.conversation.runtime_generation != self.runtime_generation
            || sandbox.revision < self.sandbox.expected_revision
            || workspace.revision < self.workspace.revision
            || workspace.next_worktree_number < self.workspace.next_worktree_number
            || worktree.revision < self.worktree.revision
            || sandbox.lifecycle.home != self.home
        {
            return Err(SandboxStoreError::Conflict);
        }
        let mut original = self.sandbox.clone();
        original.expected_revision = sandbox.revision;
        let mut workspace = workspace.clone();
        workspace.revision = self.workspace.revision;
        workspace.next_worktree_number = self.workspace.next_worktree_number;
        let mut worktree = worktree.clone();
        worktree.revision = self.worktree.revision;
        if !original.matches(sandbox) || workspace != self.workspace || worktree != self.worktree {
            return Err(SandboxStoreError::Conflict);
        }
        Ok(())
    }
}

/// Stop/Delete and log-capacity cleanup have separate validation. This boundary
/// covers the ordinary result of the pending skill operation only.
pub fn requires_agent_skill_result_guard(current: &Agent, mutation: &AgentWorkMutation) -> bool {
    current.status == AgentStatus::Ready
        && current.work.step == AgentWorkStep::InstallSkill
        && mutation.agent.status == AgentStatus::Ready
        && mutation.agent.work.step == AgentWorkStep::None
        && mutation.agent.work.live_skills.last().is_some_and(|skill| {
            matches!(
                skill.status,
                AgentSkillInstallStatus::Installed | AgentSkillInstallStatus::Failed
            )
        })
}

fn eligible(
    agent: &Agent,
    sandbox: &Sandbox,
    workspace: &Workspace,
    worktree: &Worktree,
) -> Result<(), SandboxStoreError> {
    let valid = agent.status == AgentStatus::Ready
        && agent.work.step == AgentWorkStep::InstallSkill
        && agent.requested_stop_reason.is_none()
        && agent.deleted_at.is_none()
        && agent
            .work
            .live_skills
            .last()
            .is_some_and(|skill| skill.status == AgentSkillInstallStatus::Pending)
        && sandbox.id == agent.sandbox_id
        && sandbox.owner == agent.owner
        && (matches!(
            sandbox.status,
            SandboxStatus::Active | SandboxStatus::CredentialRotating
        ) || sandbox.restored_connection_is_ready(agent))
        && sandbox.lifecycle.checkpoint_capture.is_none()
        && sandbox.credential.delivered
        && sandbox.runtime.as_ref().is_some_and(|runtime| {
            runtime
                .provision_generation
                .is_none_or(|generation| agent.conversation.runtime_generation == Some(generation))
        })
        && workspace.id == agent.workspace_id
        && workspace.sandbox_id == sandbox.id
        && workspace.owner == agent.owner
        && workspace.status == WorkspaceStatus::Ready
        && worktree.id == agent.worktree_id
        && worktree.workspace_id == workspace.id
        && worktree.sandbox_id == sandbox.id
        && worktree.owner == agent.owner
        && worktree.status == WorktreeStatus::Ready
        && worktree.agent_id.as_ref() == Some(&agent.id)
        && worktree.path == format!("{}/.worktrees/{}", workspace.path, worktree.name)
        && worktree.branch == format!("cma/{}", worktree.name)
        && workspace.resolved_commit.as_ref() == Some(&worktree.base_commit);
    if valid {
        Ok(())
    } else {
        Err(SandboxStoreError::Conflict)
    }
}
