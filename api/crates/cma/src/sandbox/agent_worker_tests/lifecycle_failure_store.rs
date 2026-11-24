//! A real in-memory lifecycle store with failures at selected I/O boundaries.
use super::*;
use layer2_core::agent_ownership::Authority;
use layer2_core::sandbox::store::*;
use layer2_core::sandbox::{AgentLogEntry, Worktree};

pub(in crate::sandbox::agent_worker_tests) struct Store {
    pub inner: Arc<InMemorySandboxStore>,
    read_delays: Mutex<BTreeMap<(&'static str, usize), Duration>>,
    pub agent_reads: Mutex<Vec<AgentId>>,
    pub delayed_observations: Mutex<Vec<AgentId>>,
    pub failures: Mutex<BTreeMap<(&'static str, usize), SandboxStoreError>>,
    pub calls: Mutex<BTreeMap<&'static str, usize>>,
    pub cancel_claim: Mutex<Option<Arc<Authority>>>,
}
impl Store {
    pub fn new(inner: Arc<InMemorySandboxStore>) -> Arc<Self> {
        Arc::new(Self {
            inner,
            read_delays: Mutex::default(),
            agent_reads: Mutex::default(),
            delayed_observations: Mutex::default(),
            failures: Mutex::default(),
            calls: Mutex::default(),
            cancel_claim: Mutex::default(),
        })
    }
    pub fn fail(&self, method: &'static str, call: usize, error: SandboxStoreError) {
        self.failures.lock().unwrap().insert((method, call), error);
    }
    pub fn delay_read(&self, method: &'static str, call: usize, delay: Duration) {
        self.read_delays
            .lock()
            .unwrap()
            .insert((method, call), delay);
    }
    async fn observe_read(&self, method: &'static str) -> Result<(), SandboxStoreError> {
        let call = self.observe(method)?;
        let delay = self.read_delays.lock().unwrap().remove(&(method, call));
        if let Some(delay) = delay {
            tokio::time::sleep(delay).await;
        }
        Ok(())
    }
    fn observe(&self, method: &'static str) -> Result<usize, SandboxStoreError> {
        let mut calls = self.calls.lock().unwrap();
        let count = calls.entry(method).or_default();
        *count += 1;
        match self.failures.lock().unwrap().remove(&(method, *count)) {
            Some(error) => Err(error),
            None => Ok(*count),
        }
    }
}

#[async_trait::async_trait]
impl SandboxStore for Store {
    async fn create_sandbox(&self, sandbox: Sandbox) -> Result<(), SandboxStoreError> {
        self.observe("create_sandbox")?;
        self.inner.create_sandbox(sandbox).await
    }
    async fn get_sandbox(&self, id: &SandboxId) -> Result<Option<Sandbox>, SandboxStoreError> {
        self.observe_read("get_sandbox").await?;
        self.inner.get_sandbox(id).await
    }
    async fn save_sandbox(
        &self,
        expected_revision: u64,
        sandbox: Sandbox,
    ) -> Result<(), SandboxStoreError> {
        self.observe("save_sandbox")?;
        self.inner.save_sandbox(expected_revision, sandbox).await
    }
    async fn list_sandboxes(
        &self,
        owner: &SubjectId,
        page: &PageRequest,
    ) -> Result<Page<Sandbox>, SandboxStoreError> {
        self.observe("list_sandboxes")?;
        self.inner.list_sandboxes(owner, page).await
    }
}

#[async_trait::async_trait]
impl WorkspaceStore for Store {
    async fn create_workspace(&self, workspace: Workspace) -> Result<(), SandboxStoreError> {
        self.observe("create_workspace")?;
        self.inner.create_workspace(workspace).await
    }
    async fn get_workspace(
        &self,
        id: &WorkspaceId,
    ) -> Result<Option<Workspace>, SandboxStoreError> {
        self.observe("get_workspace")?;
        self.inner.get_workspace(id).await
    }
    async fn save_workspace(
        &self,
        expected_revision: u64,
        workspace: Workspace,
    ) -> Result<(), SandboxStoreError> {
        self.observe("save_workspace")?;
        self.inner
            .save_workspace(expected_revision, workspace)
            .await
    }
    async fn list_workspaces(
        &self,
        sandbox_id: &SandboxId,
        page: &PageRequest,
    ) -> Result<Page<Workspace>, SandboxStoreError> {
        self.observe("list_workspaces")?;
        self.inner.list_workspaces(sandbox_id, page).await
    }
}

#[async_trait::async_trait]
impl WorktreeStore for Store {
    async fn create_worktree(&self, worktree: Worktree) -> Result<(), SandboxStoreError> {
        self.observe("create_worktree")?;
        self.inner.create_worktree(worktree).await
    }
    async fn get_worktree(&self, id: &WorktreeId) -> Result<Option<Worktree>, SandboxStoreError> {
        self.observe("get_worktree")?;
        self.inner.get_worktree(id).await
    }
    async fn save_worktree(
        &self,
        expected_revision: u64,
        worktree: Worktree,
    ) -> Result<(), SandboxStoreError> {
        self.observe("save_worktree")?;
        self.inner.save_worktree(expected_revision, worktree).await
    }
    async fn list_worktrees(
        &self,
        workspace_id: &WorkspaceId,
        page: &PageRequest,
    ) -> Result<Page<Worktree>, SandboxStoreError> {
        self.observe("list_worktrees")?;
        self.inner.list_worktrees(workspace_id, page).await
    }
}

#[async_trait::async_trait]
impl AgentStore for Store {
    async fn create_agent(&self, agent: Agent) -> Result<(), SandboxStoreError> {
        self.observe("create_agent")?;
        self.inner.create_agent(agent).await
    }
    async fn get_agent(&self, id: &AgentId) -> Result<Option<Agent>, SandboxStoreError> {
        self.agent_reads.lock().unwrap().push(id.clone());
        self.observe_read("get_agent").await?;
        if self.delayed_observations.lock().unwrap().contains(id) {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }
        self.inner.get_agent(id).await
    }
    async fn save_agent(
        &self,
        expected_revision: u64,
        agent: Agent,
    ) -> Result<(), SandboxStoreError> {
        self.observe("save_agent")?;
        self.inner.save_agent(expected_revision, agent).await
    }
    async fn list_agents(
        &self,
        owner: &SubjectId,
        page: &PageRequest,
    ) -> Result<Page<Agent>, SandboxStoreError> {
        self.observe("list_agents")?;
        self.inner.list_agents(owner, page).await
    }
}

#[async_trait::async_trait]
impl AgentLogStore for Store {
    async fn append_agent_log(
        &self,
        agent_id: &AgentId,
        appended_at: String,
        kind: AgentLogKind,
    ) -> Result<AgentLogEntry, SandboxStoreError> {
        self.observe("append_agent_log")?;
        self.inner
            .append_agent_log(agent_id, appended_at, kind)
            .await
    }
    async fn agent_log(
        &self,
        agent_id: &AgentId,
        after: u64,
        limit: u16,
    ) -> Result<Vec<AgentLogEntry>, SandboxStoreError> {
        self.observe("agent_log")?;
        self.inner.agent_log(agent_id, after, limit).await
    }
}

#[async_trait::async_trait]
impl AgentWorkerStore for Store {
    async fn live_agent_connections(
        &self,
        page: &PageRequest,
    ) -> Result<Page<AgentConnectionRecoveryCandidate>, SandboxStoreError> {
        self.observe("live_agent_connections")?;
        self.inner.live_agent_connections(page).await
    }
    async fn schedule_agent_connection_recovery(
        &self,
        request: AgentConnectionRecoveryRequest,
    ) -> Result<AgentConnectionRecoveryOutcome, SandboxStoreError> {
        self.observe("schedule_agent_connection_recovery")?;
        self.inner.schedule_agent_connection_recovery(request).await
    }
    async fn due_agent_work_filtered(
        &self,
        now: &str,
        limit: u16,
        filter: AgentWorkDiscovery<'_>,
    ) -> Result<Vec<AgentId>, SandboxStoreError> {
        self.observe("due_agent_work")?;
        self.inner.due_agent_work_filtered(now, limit, filter).await
    }
    async fn claim_agent_work(
        &self,
        id: &AgentId,
        holder: &str,
        now: String,
        lease_expires_at: String,
    ) -> Result<Option<Agent>, SandboxStoreError> {
        self.observe("claim_agent_work")?;
        let result = self
            .inner
            .claim_agent_work(id, holder, now, lease_expires_at)
            .await;
        let cancel = self.cancel_claim.lock().unwrap().take();
        if let Some(authority) = cancel {
            authority.revoke();
            std::future::pending::<()>().await;
        }
        result
    }
    async fn commit_agent_work(
        &self,
        mutation: AgentWorkMutation,
    ) -> Result<(), SandboxStoreError> {
        self.observe("commit_agent_work")?;
        self.inner.commit_agent_work(mutation).await
    }
}
