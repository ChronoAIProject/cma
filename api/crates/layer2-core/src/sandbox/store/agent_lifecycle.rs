use async_trait::async_trait;

use crate::ids::AgentId;
use crate::sandbox::{Agent, AgentLogKind, SandboxEventKind, Worktree};

use super::SandboxStoreError;

#[derive(Debug, Clone)]
pub struct VersionedWorktree {
    pub expected_revision: u64,
    pub worktree: Worktree,
}

/// One fenced, all-or-nothing agent lifecycle mutation.
#[derive(Debug, Clone)]
pub struct AgentWorkMutation {
    pub expected_agent_revision: u64,
    pub holder: String,
    pub fence: u64,
    pub agent: Agent,
    pub worktree: Option<VersionedWorktree>,
    pub appended_at: String,
    pub sandbox_event: Option<SandboxEventKind>,
    pub agent_logs: Vec<AgentLogKind>,
}

/// Durable agent work discovery, lease fencing, and atomic publication.
#[async_trait]
pub trait AgentWorkerStore: Send + Sync {
    async fn due_agent_work(
        &self,
        now: &str,
        limit: u16,
    ) -> Result<Vec<AgentId>, SandboxStoreError>;

    async fn claim_agent_work(
        &self,
        id: &AgentId,
        holder: &str,
        now: String,
        lease_expires_at: String,
    ) -> Result<Option<Agent>, SandboxStoreError>;

    async fn commit_agent_work(&self, mutation: AgentWorkMutation)
        -> Result<(), SandboxStoreError>;
}
