use super::*;

/// Public shared metadata is deliberately distinct from owner configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SandboxHierarchyReading {
    Personal(Box<SandboxHierarchyView>),
    Shared(SharedSandboxHierarchy),
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceHierarchyReading {
    Personal(Box<WorkspaceHierarchyView>),
    Shared(Box<SharedWorkspaceHierarchy>),
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorktreeHierarchyReading {
    Personal(WorktreeHierarchySummary),
    Shared(SharedWorktreeSummary),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SandboxInventoryReading {
    Personal(Box<SandboxHierarchySummary>),
    Shared(SharedSandboxSummary),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedSandboxSummary {
    pub id: SandboxId,
    pub name: String,
    pub status: SandboxStatus,
    pub created_at: String,
    pub updated_at: String,
    pub last_activity_at: String,
    pub retired_at: Option<String>,
    pub counts: HierarchyCounts,
}
impl From<SandboxHierarchyView> for SharedSandboxSummary {
    fn from(value: SandboxHierarchyView) -> Self {
        Self {
            id: value.sandbox.id,
            name: value.sandbox.name,
            status: value.sandbox.status,
            created_at: value.sandbox.created_at,
            updated_at: value.sandbox.updated_at,
            last_activity_at: value.sandbox.last_activity_at,
            retired_at: value.sandbox.retired_at,
            counts: value.counts,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedSandboxHierarchy {
    pub id: SandboxId,
    pub revision: u64,
    pub name: String,
    pub status: SandboxStatus,
    pub created_at: String,
    pub updated_at: String,
    pub last_activity_at: String,
    pub retired_at: Option<String>,
    pub counts: HierarchyCounts,
    pub workspaces: Vec<SharedWorkspaceSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedWorkspaceSummary {
    pub id: WorkspaceId,
    pub sandbox_id: SandboxId,
    pub name: String,
    pub status: WorkspaceStatus,
    pub counts: HierarchyCounts,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedWorkspaceHierarchy {
    pub id: WorkspaceId,
    pub sandbox_id: SandboxId,
    pub name: String,
    pub status: WorkspaceStatus,
    pub created_at: String,
    pub ready_at: Option<String>,
    pub removed_at: Option<String>,
    pub counts: HierarchyCounts,
    pub worktrees: Vec<SharedWorktreeSummary>,
    pub agents: Vec<SharedHierarchyAgent>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedWorktreeSummary {
    pub id: WorktreeId,
    pub workspace_id: WorkspaceId,
    pub sandbox_id: SandboxId,
    pub name: String,
    pub status: WorktreeStatus,
    pub agent_id: Option<AgentId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedHierarchyAgent {
    pub id: AgentId,
    pub worktree_id: WorktreeId,
    pub workspace_id: WorkspaceId,
    pub sandbox_id: SandboxId,
    pub status: AgentStatus,
    pub title: Option<String>,
    pub updated_at: String,
}

impl From<SandboxHierarchyView> for SharedSandboxHierarchy {
    fn from(value: SandboxHierarchyView) -> Self {
        let sandbox = value.sandbox;
        Self {
            id: sandbox.id,
            revision: sandbox.revision,
            name: sandbox.name,
            status: sandbox.status,
            created_at: sandbox.created_at,
            updated_at: sandbox.updated_at,
            last_activity_at: sandbox.last_activity_at,
            retired_at: sandbox.retired_at,
            counts: value.counts,
            workspaces: value.workspaces.into_iter().map(Into::into).collect(),
        }
    }
}
impl From<WorkspaceHierarchySummary> for SharedWorkspaceSummary {
    fn from(value: WorkspaceHierarchySummary) -> Self {
        Self {
            id: value.id,
            sandbox_id: value.sandbox_id,
            name: value.name,
            status: value.status,
            counts: value.counts,
        }
    }
}
impl From<WorkspaceHierarchyView> for SharedWorkspaceHierarchy {
    fn from(value: WorkspaceHierarchyView) -> Self {
        let workspace = value.workspace;
        Self {
            id: workspace.id,
            sandbox_id: workspace.sandbox_id,
            name: workspace.name,
            status: workspace.status,
            created_at: workspace.created_at,
            ready_at: workspace.ready_at,
            removed_at: workspace.removed_at,
            counts: value.counts,
            worktrees: value.worktrees.into_iter().map(Into::into).collect(),
            agents: value
                .agents
                .into_iter()
                .map(|agent| SharedHierarchyAgent {
                    id: agent.id,
                    worktree_id: agent.worktree_id,
                    workspace_id: agent.workspace_id,
                    sandbox_id: agent.sandbox_id,
                    status: agent.status,
                    title: agent.owner_title.or(agent.title),
                    updated_at: agent.updated_at,
                })
                .collect(),
        }
    }
}
impl From<WorktreeHierarchySummary> for SharedWorktreeSummary {
    fn from(value: WorktreeHierarchySummary) -> Self {
        Self {
            id: value.id,
            workspace_id: value.workspace_id,
            sandbox_id: value.sandbox_id,
            name: value.name,
            status: value.status,
            agent_id: value.agent_id,
        }
    }
}
