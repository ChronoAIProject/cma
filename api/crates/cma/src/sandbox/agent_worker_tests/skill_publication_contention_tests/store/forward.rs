use super::*;

// Only the six lifecycle interfaces are decorated. Admission, operations and
// recording retain the shared real store, rather than a second fake database.
macro_rules! forward {
    ($trait:ident { $(fn $name:ident($($arg:ident: $ty:ty),*) -> $ret:ty;)* }) => {
        #[async_trait::async_trait]
        impl $trait for PublicationStore {
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
#[async_trait::async_trait]
impl AgentStore for PublicationStore {
    async fn create_agent(&self, agent: Agent) -> Result<(), SandboxStoreError> {
        self.inner.create_agent(agent).await
    }
    async fn get_agent(&self, id: &AgentId) -> Result<Option<Agent>, SandboxStoreError> {
        let mut agent = self.inner.get_agent(id).await?;
        if self.attempts.load(Ordering::SeqCst) > 0 {
            if matches!(self.fault, Fault::ReadbackUnavailable) {
                return Err(SandboxStoreError::Unavailable(
                    "publication readback unavailable".into(),
                ));
            }
            // Exercise fail-closed handling of a successor snapshot without
            // weakening the real store's immutable skill admission checks.
            if let Some(agent) = &mut agent {
                match self.fault {
                    Fault::Abort(Change::Intent) => {
                        agent.work.live_skills.last_mut().unwrap().reference.version =
                            Some("8.0".into());
                        agent
                            .work
                            .live_skills
                            .last_mut()
                            .unwrap()
                            .material
                            .as_mut()
                            .unwrap()
                            .version = "8.0".into();
                        agent.revision += 1;
                    }
                    Fault::Abort(Change::Settings) => {
                        let mut settings = agent.settings();
                        settings.system_prompt = Some("Replacement owner prompt".into());
                        agent.conversation.settings = Some(settings);
                        agent.conversation.preferences.revision += 1;
                        agent.revision += 1;
                    }
                    Fault::Abort(Change::Material) => {
                        agent
                            .work
                            .live_skills
                            .last_mut()
                            .unwrap()
                            .material
                            .as_mut()
                            .unwrap()
                            .archive_sha256 = "b".repeat(64);
                        agent.revision += 1;
                    }
                    _ => {}
                }
            }
        }
        Ok(agent)
    }
    async fn save_agent(&self, revision: u64, agent: Agent) -> Result<(), SandboxStoreError> {
        self.inner.save_agent(revision, agent).await
    }
    async fn list_agents(
        &self,
        owner: &SubjectId,
        page: &PageRequest,
    ) -> Result<Page<Agent>, SandboxStoreError> {
        self.inner.list_agents(owner, page).await
    }
    async fn require_legacy_first_message(&self, agent: &Agent) -> Result<(), SandboxStoreError> {
        self.inner.require_legacy_first_message(agent).await
    }
}

forward!(AgentLogStore {
    fn append_agent_log(agent_id: &AgentId, appended_at: String, kind: AgentLogKind) -> Result<AgentLogEntry, SandboxStoreError>;
    fn agent_log(agent_id: &AgentId, after: u64, limit: u16) -> Result<Vec<AgentLogEntry>, SandboxStoreError>;
});
