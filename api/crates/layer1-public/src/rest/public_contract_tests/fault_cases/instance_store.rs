use async_trait::async_trait;
use layer2_core::sandbox::store::*;
use layer2_core::sandbox::*;
use layer2_core::{AgentId, SandboxId, SubjectId, WorkspaceId, WorktreeId};

pub(super) struct UnavailableInstances;

macro_rules! unavailable_port {
    ($port:ident { $(fn $method:ident($($arg:ident: $kind:ty),*) -> $result:ty;)* }) => {
        #[async_trait]
        impl $port for UnavailableInstances {
            $(async fn $method(&self, $($arg: $kind),*) -> Result<$result, SandboxStoreError> {
                let _ = ($($arg,)*);
                Err(SandboxStoreError::Unavailable("contract instance storage unavailable".into()))
            })*
        }
    };
}

unavailable_port!(SandboxStore {
    fn create_sandbox(value: Sandbox) -> ();
    fn get_sandbox(id: &SandboxId) -> Option<Sandbox>;
    fn save_sandbox(revision: u64, value: Sandbox) -> ();
    fn list_sandboxes(owner: &SubjectId, page: &PageRequest) -> Page<Sandbox>;
});
unavailable_port!(WorkspaceStore {
    fn create_workspace(value: Workspace) -> ();
    fn get_workspace(id: &WorkspaceId) -> Option<Workspace>;
    fn save_workspace(revision: u64, value: Workspace) -> ();
    fn list_workspaces(id: &SandboxId, page: &PageRequest) -> Page<Workspace>;
});
unavailable_port!(WorktreeStore {
    fn create_worktree(value: Worktree) -> ();
    fn get_worktree(id: &WorktreeId) -> Option<Worktree>;
    fn save_worktree(revision: u64, value: Worktree) -> ();
    fn list_worktrees(id: &WorkspaceId, page: &PageRequest) -> Page<Worktree>;
});
unavailable_port!(AgentStore {
    fn create_agent(value: Agent) -> ();
    fn get_agent(id: &AgentId) -> Option<Agent>;
    fn save_agent(revision: u64, value: Agent) -> ();
    fn list_agents(owner: &SubjectId, page: &PageRequest) -> Page<Agent>;
});
unavailable_port!(SandboxEventStore {
    fn append_sandbox_event(id: &SandboxId, at: String, kind: SandboxEventKind) -> SandboxEvent;
    fn sandbox_events(id: &SandboxId, after: u64, limit: u16) -> Vec<SandboxEvent>;
});
unavailable_port!(AgentLogStore {
    fn append_agent_log(id: &AgentId, at: String, kind: AgentLogKind) -> AgentLogEntry;
    fn agent_log(id: &AgentId, after: u64, limit: u16) -> Vec<AgentLogEntry>;
});
unavailable_port!(AgentReadQueryStore {
    fn list_owned_agents(owner: &SubjectId, query: &AgentListQuery, page: &PageRequest) -> Page<Agent>;
});
unavailable_port!(HierarchyReadStore {
    fn read_snapshot_archive(sandbox_id: &SandboxId, checkpoint: &SandboxCheckpointRef) -> SandboxSnapshotDownload;
    fn list_owner_sandbox_inventory(owner: &SubjectId, status: Option<SandboxStatus>, page: &PageRequest) -> Page<SandboxInventoryRecord>;
    fn read_sandbox_hierarchy(id: &SandboxId) -> Option<SandboxHierarchyRecord>;
    fn read_workspace_hierarchy(id: &WorkspaceId) -> Option<WorkspaceHierarchyRecord>;
    fn read_hierarchy_worktree(id: &WorktreeId) -> Option<Worktree>;
});
unavailable_port!(SandboxTransactionStore {
    fn commit(transaction: SandboxTransaction) -> ();
});
unavailable_port!(SandboxAdmissionStore {
    fn sandbox_admission_replay(owner: &SubjectId, origin: &layer2_core::sandbox::SandboxOrigin) -> Option<SandboxAdmissionReplay>;
    fn admit_sandbox(admission: SandboxAdmission) -> SandboxAdmissionResult;
});
unavailable_port!(WorkspaceAdmissionStore {
    fn admit_workspace(admission: WorkspaceAdmission) -> WorkspaceAdmissionResult;
    fn begin_workspace_removal(id: &WorkspaceId, owner: &SubjectId, now: String) -> Workspace;
});
unavailable_port!(AgentAdmissionStore {
    fn admit_agent(admission: AgentAdmission) -> AgentAdmissionResult;
});
unavailable_port!(AgentConversationStore {
    fn agent_message_checkpoint(id: &AgentId) -> Option<AgentMessageCheckpoint>;
    fn commit_agent_message_update(current: Agent, previous: Option<AgentMessageCheckpoint>, next: AgentMessageCheckpoint, mutation: Option<AgentConversationMutation>) -> AgentUpdateOutcome;
    fn commit_agent_conversation(mutation: AgentConversationMutation) -> Vec<AgentLogEntry>;
    fn pending_agent_approvals(id: &AgentId) -> Vec<AgentLogEntry>;
});
unavailable_port!(SandboxLifecycleExecutionStore {
    fn sandbox_agents(id: &SandboxId, page: &PageRequest) -> Page<Agent>;
    fn publish_sandbox_checkpoint(mutation: SandboxCheckpointMutation) -> ();
    fn commit_sandbox_lifecycle(mutation: SandboxLifecycleMutation) -> ();
});
unavailable_port!(SandboxAdminStore {
    fn list_admin_sandboxes(query: &SandboxAdminQuery) -> Page<SandboxAdminSummary>;
    fn read_admin_sandbox(id: &SandboxId) -> Option<SandboxAdminView>;
    fn read_admin_sandbox_events(id: &SandboxId, after: u64, limit: u16) -> Vec<SandboxEvent>;
});

#[async_trait]
impl AgentControlStore for UnavailableInstances {
    async fn schedule_agent_stop(
        &self,
        _: AgentStopRequest,
    ) -> Result<AgentControlResult, AgentControlStoreError> {
        Err(AgentControlStoreError::Unavailable(
            "contract control storage unavailable".into(),
        ))
    }

    async fn schedule_agent_delete(
        &self,
        _: AgentDeleteRequest,
    ) -> Result<AgentControlResult, AgentControlStoreError> {
        Err(AgentControlStoreError::Unavailable(
            "contract control storage unavailable".into(),
        ))
    }
}
