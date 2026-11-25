use super::*;
use crate::sandbox::store::{AgentReadQueryStore, Page};

pub(super) struct CountedLog {
    pub inner: Arc<InMemorySandboxStore>,
    pub calls: AtomicUsize,
    pub delay: Duration,
    pub gate: Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>,
    pub fail: bool,
}

macro_rules! delegate_entity {
    ($store:ident, $entity:ty, $id:ty, $create:ident, $get:ident, $save:ident, $list:ident, $parent:ty) => {
        #[async_trait]
        impl $store for CountedLog {
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
                unreachable!("response observation listed entities")
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
impl AgentReadQueryStore for CountedLog {
    async fn list_owned_agents(
        &self,
        _: &SubjectId,
        _: &AgentListQuery,
        _: &PageRequest,
    ) -> Result<Page<Agent>, SandboxStoreError> {
        unreachable!("response observation listed Agents")
    }
}

#[async_trait]
impl AgentLogStore for CountedLog {
    async fn append_agent_log(
        &self,
        _: &AgentId,
        _: String,
        _: AgentLogKind,
    ) -> Result<AgentLogEntry, SandboxStoreError> {
        panic!("response observation wrote history")
    }
    async fn agent_log(
        &self,
        id: &AgentId,
        after: u64,
        limit: u16,
    ) -> Result<Vec<AgentLogEntry>, SandboxStoreError> {
        self.inner.agent_log(id, after, limit).await
    }
    async fn agent_log_entry(
        &self,
        id: &AgentId,
        seq: u64,
    ) -> Result<Option<AgentLogEntry>, SandboxStoreError> {
        self.inner.agent_log_entry(id, seq).await
    }
    async fn agent_log_prefix(
        &self,
        id: &AgentId,
        through: u64,
    ) -> Result<Vec<AgentLogEntry>, SandboxStoreError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        tokio::time::sleep(self.delay).await;
        if let Some((entered, resume)) = &self.gate {
            entered.notify_one();
            resume.notified().await;
        }
        if self.fail {
            return Err(SandboxStoreError::Unavailable(
                "private full source error".into(),
            ));
        }
        self.inner.agent_log_prefix(id, through).await
    }
}
