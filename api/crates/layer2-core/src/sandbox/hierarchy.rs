use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::{AgentId, Caller, Capability, SandboxId, SubjectId, WorkspaceId, WorktreeId};

use super::store::{Page, PageRequest, SandboxStoreError};
use super::{
    Agent, AgentStatus, Sandbox, SandboxStatus, Workspace, WorkspaceStatus, Worktree,
    WorktreeStatus,
};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OwnerSandboxQuery {
    pub status: Option<SandboxStatus>,
    pub page: PageRequest,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HierarchyCounts {
    pub workspaces: u64,
    pub worktrees: u64,
    pub agents: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxInventoryRecord {
    pub sandbox: Sandbox,
    pub counts: HierarchyCounts,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxHierarchyRecord {
    pub sandbox: Sandbox,
    pub workspaces: Vec<Workspace>,
    pub worktrees: Vec<Worktree>,
    pub agents: Vec<Agent>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceHierarchyRecord {
    pub workspace: Workspace,
    pub worktrees: Vec<Worktree>,
    pub agents: Vec<Agent>,
}

#[async_trait]
pub trait HierarchyReadStore: Send + Sync {
    async fn list_owner_sandbox_inventory(
        &self,
        owner: &SubjectId,
        status: Option<SandboxStatus>,
        page: &PageRequest,
    ) -> Result<Page<SandboxInventoryRecord>, SandboxStoreError>;

    async fn read_sandbox_hierarchy(
        &self,
        id: &SandboxId,
    ) -> Result<Option<SandboxHierarchyRecord>, SandboxStoreError>;

    async fn read_workspace_hierarchy(
        &self,
        id: &WorkspaceId,
    ) -> Result<Option<WorkspaceHierarchyRecord>, SandboxStoreError>;

    async fn read_hierarchy_worktree(
        &self,
        id: &WorktreeId,
    ) -> Result<Option<Worktree>, SandboxStoreError>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxHierarchySummary {
    pub sandbox: Sandbox,
    pub counts: HierarchyCounts,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceHierarchySummary {
    pub id: WorkspaceId,
    pub sandbox_id: SandboxId,
    pub name: String,
    pub status: WorkspaceStatus,
    pub profile_id: String,
    pub profile_revision: u32,
    pub counts: HierarchyCounts,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorktreeHierarchySummary {
    pub id: WorktreeId,
    pub workspace_id: WorkspaceId,
    pub sandbox_id: SandboxId,
    pub name: String,
    pub path: String,
    pub branch: String,
    pub base_commit: String,
    pub status: WorktreeStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<AgentId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentHierarchySummary {
    pub id: AgentId,
    pub worktree_id: WorktreeId,
    pub workspace_id: WorkspaceId,
    pub sandbox_id: SandboxId,
    pub status: AgentStatus,
    pub profile_id: String,
    pub profile_revision: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxHierarchyView {
    pub sandbox: Sandbox,
    pub workspaces: Vec<WorkspaceHierarchySummary>,
    pub counts: HierarchyCounts,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceHierarchyView {
    pub workspace: Workspace,
    pub worktrees: Vec<WorktreeHierarchySummary>,
    pub agents: Vec<AgentHierarchySummary>,
    pub counts: HierarchyCounts,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HierarchyReadError {
    #[error("forbidden")]
    Forbidden,
    #[error("resource not found")]
    NotFound,
    #[error("invalid hierarchy page")]
    InvalidPage,
    #[error("hierarchy data is not usable: {0}")]
    Corrupt(String),
    #[error("hierarchy dependency is unavailable: {0}")]
    Unavailable(String),
}

impl HierarchyReadError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Forbidden => "forbidden",
            Self::NotFound => "resource_not_found",
            Self::InvalidPage => "invalid_page",
            Self::Corrupt(_) => "stored_hierarchy_corrupt",
            Self::Unavailable(_) => "hierarchy_service_unavailable",
        }
    }
}

pub struct HierarchyReadService {
    store: Arc<dyn HierarchyReadStore>,
}

impl HierarchyReadService {
    pub fn new<S>(store: Arc<S>) -> Arc<Self>
    where
        S: HierarchyReadStore + 'static,
    {
        Arc::new(Self { store })
    }

    pub async fn list_sandboxes(
        &self,
        caller: &Caller,
        query: &OwnerSandboxQuery,
    ) -> Result<Page<SandboxHierarchySummary>, HierarchyReadError> {
        require(caller)?;
        PageRequest::new(query.page.limit, query.page.cursor.clone()).map_err(store_error)?;
        let stored = self
            .store
            .list_owner_sandbox_inventory(&caller.subject, query.status, &query.page)
            .await
            .map_err(store_error)?;
        let mut items = Vec::with_capacity(stored.items.len());
        for record in stored.items {
            if record.sandbox.owner != caller.subject
                || query
                    .status
                    .is_some_and(|status| record.sandbox.status != status)
            {
                return Err(corrupt("owner sandbox inventory escaped its query"));
            }
            items.push(SandboxHierarchySummary {
                sandbox: record.sandbox,
                counts: record.counts,
            });
        }
        Ok(Page {
            items,
            next: stored.next,
        })
    }

    pub async fn sandbox(
        &self,
        caller: &Caller,
        id: &SandboxId,
    ) -> Result<SandboxHierarchyView, HierarchyReadError> {
        require(caller)?;
        let record = self
            .store
            .read_sandbox_hierarchy(id)
            .await
            .map_err(store_error)?
            .filter(|record| record.sandbox.owner == caller.subject)
            .ok_or(HierarchyReadError::NotFound)?;
        sandbox_view(record)
    }

    pub async fn workspaces(
        &self,
        caller: &Caller,
        id: &SandboxId,
    ) -> Result<Vec<WorkspaceHierarchySummary>, HierarchyReadError> {
        Ok(self.sandbox(caller, id).await?.workspaces)
    }

    pub async fn workspace(
        &self,
        caller: &Caller,
        id: &WorkspaceId,
    ) -> Result<WorkspaceHierarchyView, HierarchyReadError> {
        require(caller)?;
        let record = self
            .store
            .read_workspace_hierarchy(id)
            .await
            .map_err(store_error)?
            .filter(|record| record.workspace.owner == caller.subject)
            .ok_or(HierarchyReadError::NotFound)?;
        workspace_view(record)
    }

    pub async fn worktrees(
        &self,
        caller: &Caller,
        id: &WorkspaceId,
    ) -> Result<Vec<WorktreeHierarchySummary>, HierarchyReadError> {
        Ok(self.workspace(caller, id).await?.worktrees)
    }

    pub async fn worktree(
        &self,
        caller: &Caller,
        id: &WorktreeId,
    ) -> Result<WorktreeHierarchySummary, HierarchyReadError> {
        require(caller)?;
        let worktree = self
            .store
            .read_hierarchy_worktree(id)
            .await
            .map_err(store_error)?
            .filter(|worktree| worktree.owner == caller.subject)
            .ok_or(HierarchyReadError::NotFound)?;
        Ok(WorktreeHierarchySummary::from(&worktree))
    }
}

impl From<&Worktree> for WorktreeHierarchySummary {
    fn from(value: &Worktree) -> Self {
        Self {
            id: value.id.clone(),
            workspace_id: value.workspace_id.clone(),
            sandbox_id: value.sandbox_id.clone(),
            name: value.name.clone(),
            path: value.path.clone(),
            branch: value.branch.clone(),
            base_commit: value.base_commit.clone(),
            status: value.status,
            agent_id: value.agent_id.clone(),
        }
    }
}

impl From<&Agent> for AgentHierarchySummary {
    fn from(value: &Agent) -> Self {
        Self {
            id: value.id.clone(),
            worktree_id: value.worktree_id.clone(),
            workspace_id: value.workspace_id.clone(),
            sandbox_id: value.sandbox_id.clone(),
            status: value.status,
            profile_id: value.profile.id.to_string(),
            profile_revision: value.profile.revision,
            title: value.title.clone(),
            updated_at: value.updated_at.clone(),
        }
    }
}

fn sandbox_view(
    record: SandboxHierarchyRecord,
) -> Result<SandboxHierarchyView, HierarchyReadError> {
    let sandbox_id = &record.sandbox.id;
    let owner = &record.sandbox.owner;
    if record
        .workspaces
        .iter()
        .any(|workspace| &workspace.sandbox_id != sandbox_id || &workspace.owner != owner)
        || record
            .worktrees
            .iter()
            .any(|worktree| &worktree.sandbox_id != sandbox_id || &worktree.owner != owner)
        || record
            .agents
            .iter()
            .any(|agent| &agent.sandbox_id != sandbox_id || &agent.owner != owner)
    {
        return Err(corrupt("sandbox child lineage does not match"));
    }
    let workspace_ids: HashSet<_> = record.workspaces.iter().map(|value| &value.id).collect();
    if record
        .worktrees
        .iter()
        .any(|value| !workspace_ids.contains(&value.workspace_id))
        || record
            .agents
            .iter()
            .any(|value| !workspace_ids.contains(&value.workspace_id))
    {
        return Err(corrupt("sandbox child references an unknown workspace"));
    }
    validate_agent_worktrees(&record.worktrees, &record.agents)?;
    let mut workspaces: Vec<_> = record
        .workspaces
        .iter()
        .map(|workspace| workspace_summary(workspace, &record.worktrees, &record.agents))
        .collect();
    workspaces.sort_by(|left, right| left.id.as_str().cmp(right.id.as_str()));
    Ok(SandboxHierarchyView {
        counts: counts(
            record.workspaces.len(),
            record.worktrees.len(),
            record.agents.len(),
        ),
        sandbox: record.sandbox,
        workspaces,
    })
}

fn workspace_view(
    record: WorkspaceHierarchyRecord,
) -> Result<WorkspaceHierarchyView, HierarchyReadError> {
    if record.worktrees.iter().any(|worktree| {
        worktree.workspace_id != record.workspace.id
            || worktree.sandbox_id != record.workspace.sandbox_id
            || worktree.owner != record.workspace.owner
    }) || record.agents.iter().any(|agent| {
        agent.workspace_id != record.workspace.id
            || agent.sandbox_id != record.workspace.sandbox_id
            || agent.owner != record.workspace.owner
    }) {
        return Err(corrupt("workspace child lineage does not match"));
    }
    validate_agent_worktrees(&record.worktrees, &record.agents)?;
    let mut worktrees: Vec<_> = record
        .worktrees
        .iter()
        .map(WorktreeHierarchySummary::from)
        .collect();
    worktrees.sort_by(|left, right| left.id.as_str().cmp(right.id.as_str()));
    let mut agents: Vec<_> = record
        .agents
        .iter()
        .map(AgentHierarchySummary::from)
        .collect();
    agents.sort_by(|left, right| left.id.as_str().cmp(right.id.as_str()));
    Ok(WorkspaceHierarchyView {
        counts: counts(0, worktrees.len(), agents.len()),
        workspace: record.workspace,
        worktrees,
        agents,
    })
}

fn workspace_summary(
    workspace: &Workspace,
    worktrees: &[Worktree],
    agents: &[Agent],
) -> WorkspaceHierarchySummary {
    WorkspaceHierarchySummary {
        id: workspace.id.clone(),
        sandbox_id: workspace.sandbox_id.clone(),
        name: workspace.name.clone(),
        status: workspace.status,
        profile_id: workspace.profile.id.to_string(),
        profile_revision: workspace.profile.revision,
        counts: counts(
            0,
            worktrees
                .iter()
                .filter(|value| value.workspace_id == workspace.id)
                .count(),
            agents
                .iter()
                .filter(|value| value.workspace_id == workspace.id)
                .count(),
        ),
    }
}

fn validate_agent_worktrees(
    worktrees: &[Worktree],
    agents: &[Agent],
) -> Result<(), HierarchyReadError> {
    let by_id: HashMap<_, _> = worktrees.iter().map(|value| (&value.id, value)).collect();
    for agent in agents {
        let worktree = by_id
            .get(&agent.worktree_id)
            .ok_or_else(|| corrupt("agent references an unknown worktree"))?;
        if worktree.agent_id.as_ref() != Some(&agent.id) {
            return Err(corrupt("agent and worktree assignment disagree"));
        }
    }
    Ok(())
}

fn counts(workspaces: usize, worktrees: usize, agents: usize) -> HierarchyCounts {
    HierarchyCounts {
        workspaces: workspaces as u64,
        worktrees: worktrees as u64,
        agents: agents as u64,
    }
}

fn require(caller: &Caller) -> Result<(), HierarchyReadError> {
    caller
        .authorization
        .allows(Capability::UseOwnSandboxes)
        .then_some(())
        .ok_or(HierarchyReadError::Forbidden)
}

fn corrupt(detail: impl Into<String>) -> HierarchyReadError {
    HierarchyReadError::Corrupt(detail.into())
}

fn store_error(error: SandboxStoreError) -> HierarchyReadError {
    match error {
        SandboxStoreError::NotFound => HierarchyReadError::NotFound,
        SandboxStoreError::InvalidPage => HierarchyReadError::InvalidPage,
        SandboxStoreError::Corrupt(detail) => HierarchyReadError::Corrupt(detail),
        other => HierarchyReadError::Unavailable(other.to_string()),
    }
}
