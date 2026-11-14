use super::*;
use async_trait::async_trait;
use layer2_core::sandbox::{store::*, *};
use layer2_core::{Caller, SandboxId, WorkspaceId, WorktreeId};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

pub(super) struct Probe {
    pub inner: Arc<InMemorySandboxStore>,
    pub snapshots: AtomicUsize,
    pub logs: AtomicUsize,
    pub delay_ms: AtomicUsize,
    pub deleted: AtomicBool,
    pub failed: AtomicBool,
}
impl Probe {
    pub fn new(inner: Arc<InMemorySandboxStore>) -> Arc<Self> {
        Arc::new(Self {
            inner,
            snapshots: AtomicUsize::new(0),
            logs: AtomicUsize::new(0),
            delay_ms: AtomicUsize::new(0),
            deleted: AtomicBool::new(false),
            failed: AtomicBool::new(false),
        })
    }
    pub fn reads(self: &Arc<Self>) -> Arc<AgentReadService> {
        AgentReadService::new(self.clone(), Arc::new(MissingResourceAccess)).with_resource_access(
            Arc::new(ResourceAccessService::new(
                self.clone(),
                Arc::new(PersonalOnly),
            )),
        )
    }
}
macro_rules! delegate_entity {
    ($store:ident, $entity:ty, $id:ty, $create:ident, $get:ident, $save:ident, $list:ident, $parent:ty) => {
        #[async_trait]
        impl $store for Probe {
            async fn $create(&self, _: $entity) -> Result<(), SandboxStoreError> {
                unreachable!("read path mutated an entity")
            }
            async fn $get(&self, id: &$id) -> Result<Option<$entity>, SandboxStoreError> {
                self.inner.$get(id).await
            }
            async fn $save(&self, _: u64, _: $entity) -> Result<(), SandboxStoreError> {
                unreachable!("read path mutated an entity")
            }
            async fn $list(
                &self,
                _: &$parent,
                _: &PageRequest,
            ) -> Result<Page<$entity>, SandboxStoreError> {
                unreachable!("event window listed entities")
            }
        }
    };
}
delegate_entity!(
    SandboxStore,
    Sandbox,
    SandboxId,
    create_sandbox,
    get_sandbox,
    save_sandbox,
    list_sandboxes,
    SubjectId
);
delegate_entity!(
    WorkspaceStore,
    Workspace,
    WorkspaceId,
    create_workspace,
    get_workspace,
    save_workspace,
    list_workspaces,
    SandboxId
);
delegate_entity!(
    WorktreeStore,
    Worktree,
    WorktreeId,
    create_worktree,
    get_worktree,
    save_worktree,
    list_worktrees,
    WorkspaceId
);
delegate_entity!(
    AgentStore,
    Agent,
    AgentId,
    create_agent,
    get_agent,
    save_agent,
    list_agents,
    SubjectId
);
#[async_trait]
impl AgentReadQueryStore for Probe {
    async fn list_owned_agents(
        &self,
        _: &SubjectId,
        _: &AgentListQuery,
        _: &PageRequest,
    ) -> Result<Page<Agent>, SandboxStoreError> {
        unreachable!("event window listed Agents")
    }
}
#[async_trait]
impl AgentLogStore for Probe {
    async fn append_agent_log(
        &self,
        _: &AgentId,
        _: String,
        _: AgentLogKind,
    ) -> Result<AgentLogEntry, SandboxStoreError> {
        unreachable!("read path appended history")
    }
    async fn agent_log(
        &self,
        id: &AgentId,
        after: u64,
        limit: u16,
    ) -> Result<Vec<AgentLogEntry>, SandboxStoreError> {
        self.logs.fetch_add(1, Ordering::Relaxed);
        self.inner.agent_log(id, after, limit).await
    }
    async fn agent_log_range(
        &self,
        id: &AgentId,
        after: u64,
        through: u64,
        limit: u16,
    ) -> Result<Vec<AgentLogEntry>, SandboxStoreError> {
        self.logs.fetch_add(1, Ordering::Relaxed);
        self.inner.agent_log_range(id, after, through, limit).await
    }
    async fn agent_log_entry(
        &self,
        id: &AgentId,
        sequence: u64,
    ) -> Result<Option<AgentLogEntry>, SandboxStoreError> {
        self.logs.fetch_add(1, Ordering::Relaxed);
        self.inner.agent_log_entry(id, sequence).await
    }
}
#[async_trait]
impl ResourceAccessStore for Probe {
    async fn resource_access_snapshot(
        &self,
        target: &ResourceTarget,
    ) -> Result<ResourceAccessSnapshot, ResourceAccessStoreError> {
        self.snapshots.fetch_add(1, Ordering::Relaxed);
        let delay = self.delay_ms.load(Ordering::Relaxed);
        if delay > 0 {
            tokio::time::sleep(Duration::from_millis(delay as u64)).await;
        }
        if self.deleted.load(Ordering::Relaxed) {
            return Err(ResourceAccessStoreError::NotFound);
        }
        if self.failed.load(Ordering::Relaxed) {
            return Err(ResourceAccessStoreError::Unavailable);
        }
        self.inner.resource_access_snapshot(target).await
    }

    async fn organization_sandboxes(
        &self,
        _: &SubjectId,
        _: &OwnerSandboxQuery,
    ) -> Result<Vec<ResourceAccessSnapshot>, ResourceAccessStoreError> {
        unreachable!("event polls do not list Sandboxes")
    }

    async fn shared_sandboxes(
        &self,
        _: &SubjectId,
        _: &[SubjectId],
        _: &OwnerSandboxQuery,
    ) -> Result<Vec<ResourceAccessSnapshot>, ResourceAccessStoreError> {
        unreachable!("event polls do not list shares")
    }

    async fn organization_agents(
        &self,
        _: &SubjectId,
        _: &AgentListQuery,
        _: &PageRequest,
    ) -> Result<Vec<ResourceAccessSnapshot>, ResourceAccessStoreError> {
        unreachable!("event polls do not list Agents")
    }
}

#[async_trait]
impl ResourceShareStore for Probe {
    async fn share_mutation_receipt(
        &self,
        _: &str,
    ) -> Result<Option<ShareMutationReceipt>, ResourceAccessStoreError> {
        unreachable!("event polls do not read share mutations")
    }

    async fn mutate_share(
        &self,
        _: ShareMutation,
    ) -> Result<ShareMutationReceipt, ResourceAccessStoreError> {
        unreachable!("event polls do not mutate shares")
    }

    async fn resource_share(
        &self,
        _: &str,
    ) -> Result<Option<ResourceShare>, ResourceAccessStoreError> {
        unreachable!("event polls use the current authorization snapshot")
    }
}

struct PersonalOnly;

#[async_trait]
impl OrganizationAuthority for PersonalOnly {
    async fn organization_context(
        &self,
        _: &Caller,
        _: &str,
        _: &SubjectId,
    ) -> Result<CallerContext, MembershipError> {
        unreachable!("personal owner needs no organization")
    }

    async fn organization_choices(
        &self,
        _: &Caller,
        _: &str,
    ) -> Result<Vec<OrganizationChoice>, MembershipError> {
        unreachable!("event polls do not list organizations")
    }
}
