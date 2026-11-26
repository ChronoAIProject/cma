use super::*;

// Delegate all unrelated operations to the same real Mongo store.
macro_rules! forward {
    ($trait:ident { $(fn $name:ident($($arg:ident: $ty:ty),*) -> $ret:ty;)* }) => {
        #[async_trait::async_trait]
        impl $trait for ProbeStore {
            $(async fn $name(&self, $($arg: $ty),*) -> $ret {
                self.inner.$name($($arg),*).await
            })*
        }
    };
}

forward!(SandboxStore {
    fn create_sandbox(sandbox: Sandbox) -> Result<(), SandboxStoreError>;
    fn get_sandbox(id: &SandboxId) -> Result<Option<Sandbox>, SandboxStoreError>;
    fn save_sandbox(expected_revision: u64, sandbox: Sandbox) -> Result<(), SandboxStoreError>;
    fn list_sandboxes(owner: &SubjectId, page: &PageRequest) -> Result<Page<Sandbox>, SandboxStoreError>;
});
forward!(WorkspaceStore {
    fn create_workspace(workspace: Workspace) -> Result<(), SandboxStoreError>;
    fn get_workspace(id: &WorkspaceId) -> Result<Option<Workspace>, SandboxStoreError>;
    fn save_workspace(expected_revision: u64, workspace: Workspace) -> Result<(), SandboxStoreError>;
    fn list_workspaces(sandbox_id: &SandboxId, page: &PageRequest) -> Result<Page<Workspace>, SandboxStoreError>;
});
forward!(WorktreeStore {
    fn create_worktree(worktree: Worktree) -> Result<(), SandboxStoreError>;
    fn get_worktree(id: &WorktreeId) -> Result<Option<Worktree>, SandboxStoreError>;
    fn save_worktree(expected_revision: u64, worktree: Worktree) -> Result<(), SandboxStoreError>;
    fn list_worktrees(workspace_id: &WorkspaceId, page: &PageRequest) -> Result<Page<Worktree>, SandboxStoreError>;
});
forward!(AgentStore {
    fn create_agent(agent: Agent) -> Result<(), SandboxStoreError>;
    fn get_agent(id: &AgentId) -> Result<Option<Agent>, SandboxStoreError>;
    fn save_agent(revision: u64, agent: Agent) -> Result<(), SandboxStoreError>;
    fn list_agents(owner: &SubjectId, page: &PageRequest) -> Result<Page<Agent>, SandboxStoreError>;
    fn require_legacy_first_message(agent: &Agent) -> Result<(), SandboxStoreError>;
});

forward!(AgentLogStore {
    fn append_agent_log(agent_id: &AgentId, appended_at: String, kind: AgentLogKind) -> Result<AgentLogEntry, SandboxStoreError>;
    fn agent_log(agent_id: &AgentId, after: u64, limit: u16) -> Result<Vec<AgentLogEntry>, SandboxStoreError>;
});
