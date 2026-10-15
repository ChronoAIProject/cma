use std::sync::Arc;

use async_trait::async_trait;
use futures_util::{stream, StreamExt, TryStreamExt};
use serde::{Deserialize, Serialize};

use crate::resource_access::{ResourceAccessError, ResourceAccessService};
use crate::{AgentId, Caller, Capability, SandboxId, SubjectId, WorkspaceId, WorktreeId};

mod authorized;
mod inventory;
mod projection;
use projection::{sandbox_view, workspace_view};
mod shared;
pub use shared::*;

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
    async fn read_snapshot_archive(
        &self,
        sandbox_id: &SandboxId,
        checkpoint: &super::SandboxCheckpointRef,
    ) -> Result<SandboxSnapshotDownload, SandboxStoreError>;

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_metadata: Option<super::AgentRuntimeMetadata>,
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
    #[error(transparent)]
    Access(#[from] ResourceAccessError),
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
            Self::Access(error) => {
                use crate::resource_access::{
                    AccessError as A, MembershipError as M, ResourceAccessError as R,
                    ResourceAccessStoreError as S,
                };
                match error {
                    R::Access(A::NotFound) | R::Store(S::NotFound) => "resource_not_found",
                    R::Access(A::Forbidden) | R::Membership(M::NotMember) => "resource_forbidden",
                    R::Access(A::UnsafeInteractiveShare) | R::Store(S::UnsafeInteractiveShare) => {
                        "unsafe_interactive_share"
                    }
                    R::Store(S::Conflict) => "resource_access_changed",
                    R::Store(S::IdempotencyConflict) => "idempotency_key_reused",
                    R::Store(S::Invalid) => "invalid_share_command",
                    R::Store(S::Capacity) => "share_capacity",
                    R::Access(A::RevalidationRequired) | R::Membership(_) => {
                        "organization_authority_unavailable"
                    }
                    R::Store(S::Unavailable | S::Corrupt) => "resource_access_unavailable",
                }
            }
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
    access: Option<Arc<ResourceAccessService>>,
}

pub struct SandboxSnapshotDownload {
    pub size_bytes: u64,
    pub chunks: tokio::sync::mpsc::Receiver<Result<Vec<u8>, SandboxStoreError>>,
}

impl HierarchyReadService {
    pub async fn snapshot(
        &self,
        caller: &Caller,
        id: &SandboxId,
        epoch: u64,
    ) -> Result<SandboxSnapshotDownload, HierarchyReadError> {
        let view = self.sandbox(caller, id).await?;
        let checkpoint = view
            .sandbox
            .lifecycle
            .checkpoints
            .latest
            .as_ref()
            .filter(|checkpoint| checkpoint.epoch == epoch)
            .ok_or(HierarchyReadError::NotFound)?;
        self.store
            .read_snapshot_archive(id, checkpoint)
            .await
            .map_err(store_error)
    }

    pub fn new<S>(store: Arc<S>) -> Arc<Self>
    where
        S: HierarchyReadStore + 'static,
    {
        Arc::new(Self {
            store,
            access: None,
        })
    }

    pub fn with_resource_access(self: &Arc<Self>, access: Arc<ResourceAccessService>) -> Arc<Self> {
        Arc::new(Self {
            store: self.store.clone(),
            access: Some(access),
        })
    }

    pub async fn list_sandboxes(
        &self,
        caller: &Caller,
        query: &OwnerSandboxQuery,
    ) -> Result<Page<SandboxHierarchySummary>, HierarchyReadError> {
        tokio::time::timeout(
            std::time::Duration::from_secs(30),
            self.personal_inventory(caller, query),
        )
        .await
        .map_err(|_| HierarchyReadError::Unavailable("hierarchy inventory timed out".into()))?
    }

    async fn personal_inventory(
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
        if stored.items.len() > usize::from(query.page.limit)
            || stored.next.as_ref().is_some_and(|next| {
                stored.items.len() != usize::from(query.page.limit)
                    || stored
                        .items
                        .last()
                        .is_none_or(|record| record.sandbox.id.as_str() != next)
            })
        {
            return Err(corrupt("invalid personal inventory boundary"));
        }
        let mut previous = query.page.cursor.clone();
        for record in &stored.items {
            if record.sandbox.owner != caller.subject
                || query
                    .status
                    .is_some_and(|status| record.sandbox.status != status)
            {
                return Err(corrupt("owner sandbox inventory escaped its query"));
            }
            if previous
                .as_ref()
                .is_some_and(|id| id.as_str() >= record.sandbox.id.as_str())
            {
                return Err(corrupt("personal inventory order is invalid"));
            }
            previous = Some(record.sandbox.id.to_string());
        }
        // Bound concurrency and the whole page's deadline. Each emitted root
        // derives its counts from one validated hierarchy snapshot.
        let items = stream::iter(stored.items)
            .map(|record| async move {
                if self.access.is_some() {
                    let current = self.sandbox_personal(caller, &record.sandbox.id).await?;
                    if current.sandbox.created_at != record.sandbox.created_at
                        || query
                            .status
                            .is_some_and(|status| current.sandbox.status != status)
                    {
                        return Err(HierarchyReadError::Access(
                            crate::resource_access::ResourceAccessStoreError::Conflict.into(),
                        ));
                    }
                    Ok(SandboxHierarchySummary {
                        sandbox: current.sandbox,
                        counts: current.counts,
                    })
                } else {
                    Ok(SandboxHierarchySummary {
                        sandbox: record.sandbox,
                        counts: record.counts,
                    })
                }
            })
            .buffered(8)
            .try_collect()
            .await?;
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
        if self.access.is_some() {
            return self.sandbox_personal(caller, id).await;
        }
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
        if self.access.is_some() {
            return self.workspace_personal(caller, id).await;
        }
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
        if self.access.is_some() {
            return self.worktree_personal(caller, id).await;
        }
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
