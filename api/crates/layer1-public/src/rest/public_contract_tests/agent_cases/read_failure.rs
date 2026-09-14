use std::sync::Arc;

use async_trait::async_trait;
use layer2_core::sandbox::store::*;
use layer2_core::sandbox::*;
use layer2_core::{AgentId, SandboxId, SubjectId, WorkspaceId, WorktreeId};

use super::agents;

pub(in super::super) fn unavailable_log_reads(
    store: Arc<InMemorySandboxStore>,
) -> Arc<AgentReadService> {
    AgentReadService::new(
        Arc::new(UnavailableLog(store)),
        Arc::new(agents::TestResources),
    )
}

struct UnavailableLog(Arc<InMemorySandboxStore>);

macro_rules! delegate_port {
    ($port:ident { $(fn $method:ident($($arg:ident: $kind:ty),*) -> $result:ty;)* }) => {
        #[async_trait]
        impl $port for UnavailableLog {
            $(async fn $method(&self, $($arg: $kind),*) -> Result<$result, SandboxStoreError> {
                self.0.$method($($arg),*).await
            })*
        }
    };
}

delegate_port!(SandboxStore {
    fn create_sandbox(value: Sandbox) -> ();
    fn get_sandbox(id: &SandboxId) -> Option<Sandbox>;
    fn save_sandbox(revision: u64, value: Sandbox) -> ();
    fn list_sandboxes(owner: &SubjectId, page: &PageRequest) -> Page<Sandbox>;
});
delegate_port!(WorkspaceStore {
    fn create_workspace(value: Workspace) -> ();
    fn get_workspace(id: &WorkspaceId) -> Option<Workspace>;
    fn save_workspace(revision: u64, value: Workspace) -> ();
    fn list_workspaces(id: &SandboxId, page: &PageRequest) -> Page<Workspace>;
});
delegate_port!(WorktreeStore {
    fn create_worktree(value: Worktree) -> ();
    fn get_worktree(id: &WorktreeId) -> Option<Worktree>;
    fn save_worktree(revision: u64, value: Worktree) -> ();
    fn list_worktrees(id: &WorkspaceId, page: &PageRequest) -> Page<Worktree>;
});
delegate_port!(AgentStore {
    fn create_agent(value: Agent) -> ();
    fn get_agent(id: &AgentId) -> Option<Agent>;
    fn save_agent(revision: u64, value: Agent) -> ();
    fn list_agents(owner: &SubjectId, page: &PageRequest) -> Page<Agent>;
});
delegate_port!(AgentReadQueryStore {
    fn list_owned_agents(owner: &SubjectId, query: &AgentListQuery, page: &PageRequest) -> Page<Agent>;
});

#[async_trait]
impl AgentLogStore for UnavailableLog {
    async fn append_agent_log(
        &self,
        id: &AgentId,
        at: String,
        kind: AgentLogKind,
    ) -> Result<AgentLogEntry, SandboxStoreError> {
        self.0.append_agent_log(id, at, kind).await
    }

    async fn agent_log(
        &self,
        _: &AgentId,
        _: u64,
        _: u16,
    ) -> Result<Vec<AgentLogEntry>, SandboxStoreError> {
        Err(SandboxStoreError::Unavailable(
            "contract durable response storage unavailable".into(),
        ))
    }
}
