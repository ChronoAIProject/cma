use layer2_core::sandbox::*;
use serde::Serialize;
use utoipa::ToSchema;

use super::instances_openapi_shapes::{
    OwnerSandboxDetailView, OwnerWorkspaceSummaryView, WorkspaceDetailView, WorktreeView,
};

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
enum SharedAccess {
    Shared,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(untagged)]
pub(crate) enum AccessibleSandboxDetailView {
    Personal(Box<OwnerSandboxDetailView>),
    Shared(SharedSandboxView),
}
#[derive(Debug, Serialize, ToSchema)]
#[serde(untagged)]
pub(crate) enum AccessibleWorkspaceDetailView {
    Personal(Box<WorkspaceDetailView>),
    Shared(Box<SharedWorkspaceView>),
}
#[derive(Debug, Serialize, ToSchema)]
#[serde(untagged)]
pub(crate) enum AccessibleWorktreeView {
    Personal(WorktreeView),
    Shared(SharedWorktreeView),
}
#[derive(Debug, Serialize, ToSchema)]
#[serde(untagged)]
pub(crate) enum AccessibleWorkspaceSummaryView {
    Personal(OwnerWorkspaceSummaryView),
    Shared(SharedWorkspaceSummaryView),
}
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct AccessibleWorkspaceListView {
    pub items: Vec<AccessibleWorkspaceSummaryView>,
}
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct AccessibleWorktreeListView {
    pub items: Vec<AccessibleWorktreeView>,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct SharedSandboxView {
    access: SharedAccess,
    id: String,
    revision: u64,
    name: String,
    #[schema(value_type = String)]
    status: SandboxStatus,
    created_at: String,
    updated_at: String,
    last_activity_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    retired_at: Option<String>,
    workspace_count: u64,
    worktree_count: u64,
    agent_count: u64,
    workspaces: Vec<SharedWorkspaceSummaryView>,
}
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct SharedWorkspaceSummaryView {
    access: SharedAccess,
    id: String,
    sandbox_id: String,
    name: String,
    #[schema(value_type = String)]
    status: WorkspaceStatus,
    worktree_count: u64,
    agent_count: u64,
}
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct SharedWorkspaceView {
    access: SharedAccess,
    id: String,
    sandbox_id: String,
    name: String,
    #[schema(value_type = String)]
    status: WorkspaceStatus,
    created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    ready_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    removed_at: Option<String>,
    worktree_count: u64,
    agent_count: u64,
    worktrees: Vec<SharedWorktreeView>,
    agents: Vec<SharedHierarchyAgentView>,
}
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct SharedWorktreeView {
    access: SharedAccess,
    id: String,
    workspace_id: String,
    sandbox_id: String,
    name: String,
    #[schema(value_type = String)]
    status: WorktreeStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    agent_id: Option<String>,
}
#[derive(Debug, Serialize, ToSchema)]
struct SharedHierarchyAgentView {
    id: String,
    worktree_id: String,
    workspace_id: String,
    sandbox_id: String,
    #[schema(value_type = String)]
    status: AgentStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    updated_at: String,
}

impl From<SandboxHierarchyReading> for AccessibleSandboxDetailView {
    fn from(value: SandboxHierarchyReading) -> Self {
        match value {
            SandboxHierarchyReading::Personal(value) => Self::Personal(Box::new((*value).into())),
            SandboxHierarchyReading::Shared(value) => Self::Shared(SharedSandboxView {
                access: SharedAccess::Shared,
                id: value.id.to_string(),
                revision: value.revision,
                name: value.name,
                status: value.status,
                created_at: value.created_at,
                updated_at: value.updated_at,
                last_activity_at: value.last_activity_at,
                retired_at: value.retired_at,
                workspace_count: value.counts.workspaces,
                worktree_count: value.counts.worktrees,
                agent_count: value.counts.agents,
                workspaces: value.workspaces.into_iter().map(Into::into).collect(),
            }),
        }
    }
}
impl From<SharedWorkspaceSummary> for SharedWorkspaceSummaryView {
    fn from(value: SharedWorkspaceSummary) -> Self {
        Self {
            access: SharedAccess::Shared,
            id: value.id.to_string(),
            sandbox_id: value.sandbox_id.to_string(),
            name: value.name,
            status: value.status,
            worktree_count: value.counts.worktrees,
            agent_count: value.counts.agents,
        }
    }
}
impl From<WorkspaceHierarchyReading> for AccessibleWorkspaceDetailView {
    fn from(value: WorkspaceHierarchyReading) -> Self {
        match value {
            WorkspaceHierarchyReading::Personal(value) => Self::Personal(Box::new((*value).into())),
            WorkspaceHierarchyReading::Shared(value) => {
                Self::Shared(Box::new(SharedWorkspaceView {
                    access: SharedAccess::Shared,
                    id: value.id.to_string(),
                    sandbox_id: value.sandbox_id.to_string(),
                    name: value.name,
                    status: value.status,
                    created_at: value.created_at,
                    ready_at: value.ready_at,
                    removed_at: value.removed_at,
                    worktree_count: value.counts.worktrees,
                    agent_count: value.counts.agents,
                    worktrees: value.worktrees.into_iter().map(Into::into).collect(),
                    agents: value
                        .agents
                        .into_iter()
                        .map(|agent| SharedHierarchyAgentView {
                            id: agent.id.to_string(),
                            worktree_id: agent.worktree_id.to_string(),
                            workspace_id: agent.workspace_id.to_string(),
                            sandbox_id: agent.sandbox_id.to_string(),
                            status: agent.status,
                            title: agent.title,
                            updated_at: agent.updated_at,
                        })
                        .collect(),
                }))
            }
        }
    }
}
impl From<SharedWorktreeSummary> for SharedWorktreeView {
    fn from(value: SharedWorktreeSummary) -> Self {
        Self {
            access: SharedAccess::Shared,
            id: value.id.to_string(),
            workspace_id: value.workspace_id.to_string(),
            sandbox_id: value.sandbox_id.to_string(),
            name: value.name,
            status: value.status,
            agent_id: value.agent_id.map(|id| id.to_string()),
        }
    }
}
impl From<WorktreeHierarchyReading> for AccessibleWorktreeView {
    fn from(value: WorktreeHierarchyReading) -> Self {
        match value {
            WorktreeHierarchyReading::Personal(value) => Self::Personal(value.into()),
            WorktreeHierarchyReading::Shared(value) => Self::Shared(value.into()),
        }
    }
}
impl From<SandboxHierarchyReading> for AccessibleWorkspaceListView {
    fn from(value: SandboxHierarchyReading) -> Self {
        Self {
            items: match value {
                SandboxHierarchyReading::Personal(value) => value
                    .workspaces
                    .into_iter()
                    .map(|v| AccessibleWorkspaceSummaryView::Personal(v.into()))
                    .collect(),
                SandboxHierarchyReading::Shared(value) => value
                    .workspaces
                    .into_iter()
                    .map(|v| AccessibleWorkspaceSummaryView::Shared(v.into()))
                    .collect(),
            },
        }
    }
}
impl From<WorkspaceHierarchyReading> for AccessibleWorktreeListView {
    fn from(value: WorkspaceHierarchyReading) -> Self {
        Self {
            items: match value {
                WorkspaceHierarchyReading::Personal(value) => value
                    .worktrees
                    .into_iter()
                    .map(|v| AccessibleWorktreeView::Personal(v.into()))
                    .collect(),
                WorkspaceHierarchyReading::Shared(value) => value
                    .worktrees
                    .into_iter()
                    .map(|v| AccessibleWorktreeView::Shared(v.into()))
                    .collect(),
            },
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(untagged)]
pub(crate) enum AccessibleSandboxSummaryView {
    Personal(Box<super::instances_openapi_shapes::OwnerSandboxSummaryView>),
    Shared(SharedSandboxSummaryView),
}
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct AccessibleSandboxPageView {
    pub items: Vec<AccessibleSandboxSummaryView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
}
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct SharedSandboxSummaryView {
    access: SharedAccess,
    id: String,
    name: String,
    #[schema(value_type = String)]
    status: SandboxStatus,
    created_at: String,
    updated_at: String,
    last_activity_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    retired_at: Option<String>,
    workspace_count: u64,
    worktree_count: u64,
    agent_count: u64,
}
impl From<SandboxInventoryReading> for AccessibleSandboxSummaryView {
    fn from(value: SandboxInventoryReading) -> Self {
        match value {
            SandboxInventoryReading::Personal(value) => Self::Personal(Box::new((*value).into())),
            SandboxInventoryReading::Shared(value) => Self::Shared(SharedSandboxSummaryView {
                access: SharedAccess::Shared,
                id: value.id.to_string(),
                name: value.name,
                status: value.status,
                created_at: value.created_at,
                updated_at: value.updated_at,
                last_activity_at: value.last_activity_at,
                retired_at: value.retired_at,
                workspace_count: value.counts.workspaces,
                worktree_count: value.counts.worktrees,
                agent_count: value.counts.agents,
            }),
        }
    }
}
