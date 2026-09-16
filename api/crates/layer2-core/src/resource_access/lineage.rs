use crate::sandbox::{Agent, Sandbox, Workspace, Worktree};

use super::{ResourceAccessStoreError, ResourceTarget};

/// Ancestry captured under one store snapshot, independent of the actor. Every
/// retained parent still has to agree on ownership and exact parent identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceLineage {
    pub sandbox: Sandbox,
    pub workspace: Option<Workspace>,
    pub worktree: Option<Worktree>,
    pub agent: Option<Agent>,
}

impl ResourceLineage {
    /// Verify an assigned Agent in the same store snapshot without widening the
    /// Worktree policy target to an Agent grant or exposing foreign identifiers.
    pub fn validate_worktree_agent(&self, agent: &Agent) -> Result<(), ResourceAccessStoreError> {
        let invalid = || ResourceAccessStoreError::Corrupt;
        let workspace = self.workspace.as_ref().ok_or_else(invalid)?;
        let worktree = self.worktree.as_ref().ok_or_else(invalid)?;
        if agent.owner != self.sandbox.owner
            || agent.workspace_id != workspace.id
            || agent.worktree_id != worktree.id
            || agent.sandbox_id != self.sandbox.id
            || worktree.agent_id.as_ref() != Some(&agent.id)
            || agent.revision == 0
        {
            return Err(invalid());
        }
        agent.validate_conversation().map_err(|_| invalid())
    }

    pub fn validate(&self, target: &ResourceTarget) -> Result<(), ResourceAccessStoreError> {
        let invalid = || ResourceAccessStoreError::Corrupt;
        let sandbox = &self.sandbox;
        if sandbox.id.as_str().trim().is_empty()
            || sandbox.owner.as_str().trim().is_empty()
            || sandbox.revision == 0
        {
            return Err(invalid());
        }
        if let Some(workspace) = &self.workspace {
            if workspace.owner != sandbox.owner
                || workspace.sandbox_id != sandbox.id
                || workspace.revision == 0
            {
                return Err(invalid());
            }
        }
        if let Some(worktree) = &self.worktree {
            let workspace = self.workspace.as_ref().ok_or_else(invalid)?;
            if worktree.owner != sandbox.owner
                || worktree.workspace_id != workspace.id
                || worktree.sandbox_id != sandbox.id
                || worktree.revision == 0
            {
                return Err(invalid());
            }
        }
        if let Some(agent) = &self.agent {
            self.validate_worktree_agent(agent)?;
        }
        let matches = match target {
            ResourceTarget::Sandbox { sandbox_id } => {
                *sandbox_id == sandbox.id
                    && self.workspace.is_none()
                    && self.worktree.is_none()
                    && self.agent.is_none()
            }
            ResourceTarget::Workspace { workspace_id } => {
                self.workspace
                    .as_ref()
                    .is_some_and(|record| record.id == *workspace_id)
                    && self.worktree.is_none()
                    && self.agent.is_none()
            }
            ResourceTarget::Worktree { worktree_id } => {
                self.worktree
                    .as_ref()
                    .is_some_and(|record| record.id == *worktree_id)
                    && self.agent.is_none()
            }
            ResourceTarget::Agent { agent_id } => self
                .agent
                .as_ref()
                .is_some_and(|record| record.id == *agent_id),
        };
        if !matches {
            return Err(invalid());
        }
        Ok(())
    }
}
