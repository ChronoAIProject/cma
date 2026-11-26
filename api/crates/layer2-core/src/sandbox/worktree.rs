use serde::{Deserialize, Serialize};

use crate::ids::{AgentId, SandboxId, SubjectId, WorkspaceId, WorktreeId};

use super::{SandboxModelError, Workspace, WorkspaceStatus, WorktreeStatus};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Worktree {
    pub id: WorktreeId,
    pub workspace_id: WorkspaceId,
    pub sandbox_id: SandboxId,
    pub owner: SubjectId,
    pub name: String,
    pub path: String,
    pub branch: String,
    pub base_commit: String,
    pub status: WorktreeStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<AgentId>,
    pub revision: u64,
}

impl Worktree {
    pub fn validate_prerequisites(&self, workspace: &Workspace) -> Result<(), SandboxModelError> {
        if workspace.id != self.workspace_id
            || workspace.sandbox_id != self.sandbox_id
            || workspace.owner != self.owner
        {
            return Err(SandboxModelError::Prerequisite {
                resource: "worktree",
                detail: "workspace lineage does not match",
            });
        }
        if workspace.status != WorkspaceStatus::Ready {
            return Err(SandboxModelError::Prerequisite {
                resource: "worktree",
                detail: "workspace is not ready",
            });
        }
        let number = self
            .name
            .strip_prefix("wt-")
            .and_then(|n| n.parse::<u64>().ok());
        if number.is_none_or(|number| number == 0)
            || self.path
                != format!(
                    "{}{}/.worktrees/{}",
                    super::workspace::SANDBOX_HOME,
                    workspace.name,
                    self.name
                )
            || self.branch != format!("cma/{}", self.name)
            || self.base_commit.trim().is_empty()
            || self.revision == 0
        {
            return Err(SandboxModelError::invalid_value("worktree"));
        }
        Ok(())
    }

    pub fn transition(&mut self, next: WorktreeStatus) -> Result<(), SandboxModelError> {
        let status = self.status.transition(next)?;
        let revision = self
            .revision
            .checked_add(1)
            .ok_or_else(|| SandboxModelError::invalid_value("worktree.revision"))?;
        self.status = status;
        self.revision = revision;
        Ok(())
    }
}
