use super::*;
use layer2_core::sandbox::store::*;
use layer2_core::sandbox::{AgentLogEntry, AgentUpdateOutcome};
use std::sync::atomic::AtomicU8;
use tokio::sync::Notify;

// Faults surround the real checkpoint transaction, leaving recorder retries and
// uncertain-commit reconciliation on the production path.
pub(super) struct Store {
    pub inner: Arc<InMemorySandboxStore>,
    pub cursor: u64,
    pub fault: AtomicU8,
    pub entered: Notify,
    pub release: Notify,
}

#[async_trait::async_trait]
impl AgentStore for Store {
    async fn create_agent(&self, agent: Agent) -> Result<(), SandboxStoreError> {
        self.inner.create_agent(agent).await
    }
    async fn get_agent(&self, id: &AgentId) -> Result<Option<Agent>, SandboxStoreError> {
        self.inner.get_agent(id).await
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
}

#[async_trait::async_trait]
impl AgentLogStore for Store {
    async fn append_agent_log(
        &self,
        id: &AgentId,
        at: String,
        kind: AgentLogKind,
    ) -> Result<AgentLogEntry, SandboxStoreError> {
        self.inner.append_agent_log(id, at, kind).await
    }
    async fn agent_log(
        &self,
        id: &AgentId,
        after: u64,
        limit: u16,
    ) -> Result<Vec<AgentLogEntry>, SandboxStoreError> {
        self.inner.agent_log(id, after, limit).await
    }
}

#[async_trait::async_trait]
impl AgentConversationStore for Store {
    async fn agent_message_snapshot(
        &self,
        id: &AgentId,
    ) -> Result<Option<AgentMessageSnapshot>, SandboxStoreError> {
        self.inner.agent_message_snapshot(id).await
    }
    async fn agent_message_checkpoint(
        &self,
        id: &AgentId,
    ) -> Result<Option<AgentMessageCheckpoint>, SandboxStoreError> {
        self.inner.agent_message_checkpoint(id).await
    }
    async fn commit_agent_message_update(
        &self,
        current: Agent,
        previous: Option<AgentMessageCheckpoint>,
        next: AgentMessageCheckpoint,
        mutation: Option<AgentConversationMutation>,
    ) -> Result<AgentUpdateOutcome, SandboxStoreError> {
        let fault = if next
            .output
            .as_ref()
            .is_some_and(|source| source.cursor == self.cursor)
        {
            self.fault.swap(0, Ordering::SeqCst)
        } else {
            0
        };
        if fault != 0 {
            self.entered.notify_one();
            self.release.notified().await;
        }
        if fault == 2 {
            return Err(SandboxStoreError::Conflict);
        }
        let result = self
            .inner
            .commit_agent_message_update(current, previous, next, mutation)
            .await?;
        if fault == 3 {
            return Err(SandboxStoreError::Unavailable(
                "synthetic lost checkpoint reply".into(),
            ));
        }
        Ok(result)
    }
    async fn commit_agent_conversation(
        &self,
        mutation: AgentConversationMutation,
    ) -> Result<Vec<AgentLogEntry>, SandboxStoreError> {
        self.inner.commit_agent_conversation(mutation).await
    }
    async fn pending_agent_approvals(
        &self,
        id: &AgentId,
    ) -> Result<Vec<AgentLogEntry>, SandboxStoreError> {
        self.inner.pending_agent_approvals(id).await
    }
}

pub(super) fn install(
    h: &mut Harness,
    backend: Arc<transport::Backend>,
    cursor: u64,
    fault: u8,
) -> Arc<Store> {
    let store = Arc::new(Store {
        inner: h.store.clone(),
        cursor,
        fault: AtomicU8::new(fault),
        entered: Notify::new(),
        release: Notify::new(),
    });
    h.updates = DurableAgentUpdateRecorder::new(store.clone());
    install_worker(h, backend);
    store
}
