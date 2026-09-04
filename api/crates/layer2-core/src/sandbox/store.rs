use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use crate::ids::{AgentId, SandboxId, SubjectId, WorkspaceId, WorktreeId};

use super::{
    Agent, AgentLogEntry, AgentLogKind, Sandbox, SandboxEvent, SandboxEventKind, Workspace,
    Worktree,
};

pub const MAX_PAGE_SIZE: u16 = 200;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageRequest {
    pub limit: u16,
    pub cursor: Option<String>,
}

impl PageRequest {
    pub fn new(limit: u16, cursor: Option<String>) -> Result<Self, SandboxStoreError> {
        if limit == 0 || limit > MAX_PAGE_SIZE {
            return Err(SandboxStoreError::InvalidPage);
        }
        Ok(Self { limit, cursor })
    }
}

impl Default for PageRequest {
    fn default() -> Self {
        Self {
            limit: MAX_PAGE_SIZE,
            cursor: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SandboxStoreError {
    #[error("record not found")]
    NotFound,
    #[error("record already exists")]
    Duplicate,
    #[error("record revision changed")]
    Conflict,
    #[error("event sequence would not be gap-free")]
    SequenceConflict,
    #[error("invalid page request")]
    InvalidPage,
    #[error("transaction invariant failed: {0}")]
    InvalidTransaction(&'static str),
}

#[async_trait]
pub trait SandboxStore: Send + Sync {
    async fn create_sandbox(&self, sandbox: Sandbox) -> Result<(), SandboxStoreError>;
    async fn get_sandbox(&self, id: &SandboxId) -> Result<Option<Sandbox>, SandboxStoreError>;
    async fn save_sandbox(
        &self,
        expected_revision: u64,
        sandbox: Sandbox,
    ) -> Result<(), SandboxStoreError>;
    async fn list_sandboxes(
        &self,
        owner: &SubjectId,
        page: &PageRequest,
    ) -> Result<Page<Sandbox>, SandboxStoreError>;
}

#[async_trait]
pub trait WorkspaceStore: Send + Sync {
    async fn create_workspace(&self, workspace: Workspace) -> Result<(), SandboxStoreError>;
    async fn get_workspace(&self, id: &WorkspaceId)
        -> Result<Option<Workspace>, SandboxStoreError>;
    async fn save_workspace(
        &self,
        expected_revision: u64,
        workspace: Workspace,
    ) -> Result<(), SandboxStoreError>;
    async fn list_workspaces(
        &self,
        sandbox_id: &SandboxId,
        page: &PageRequest,
    ) -> Result<Page<Workspace>, SandboxStoreError>;
}

#[async_trait]
pub trait WorktreeStore: Send + Sync {
    async fn create_worktree(&self, worktree: Worktree) -> Result<(), SandboxStoreError>;
    async fn get_worktree(&self, id: &WorktreeId) -> Result<Option<Worktree>, SandboxStoreError>;
    async fn save_worktree(
        &self,
        expected_revision: u64,
        worktree: Worktree,
    ) -> Result<(), SandboxStoreError>;
    async fn list_worktrees(
        &self,
        workspace_id: &WorkspaceId,
        page: &PageRequest,
    ) -> Result<Page<Worktree>, SandboxStoreError>;
}

#[async_trait]
pub trait AgentStore: Send + Sync {
    async fn create_agent(&self, agent: Agent) -> Result<(), SandboxStoreError>;
    async fn get_agent(&self, id: &AgentId) -> Result<Option<Agent>, SandboxStoreError>;
    async fn save_agent(
        &self,
        expected_revision: u64,
        agent: Agent,
    ) -> Result<(), SandboxStoreError>;
    async fn list_agents(
        &self,
        owner: &SubjectId,
        page: &PageRequest,
    ) -> Result<Page<Agent>, SandboxStoreError>;
}

#[async_trait]
pub trait SandboxEventStore: Send + Sync {
    async fn append_sandbox_event(
        &self,
        sandbox_id: &SandboxId,
        appended_at: String,
        kind: SandboxEventKind,
    ) -> Result<SandboxEvent, SandboxStoreError>;
    async fn sandbox_events(
        &self,
        sandbox_id: &SandboxId,
        after: u64,
        limit: u16,
    ) -> Result<Vec<SandboxEvent>, SandboxStoreError>;
}

#[async_trait]
pub trait AgentLogStore: Send + Sync {
    async fn append_agent_log(
        &self,
        agent_id: &AgentId,
        appended_at: String,
        kind: AgentLogKind,
    ) -> Result<AgentLogEntry, SandboxStoreError>;
    async fn agent_log(
        &self,
        agent_id: &AgentId,
        after: u64,
        limit: u16,
    ) -> Result<Vec<AgentLogEntry>, SandboxStoreError>;
}

#[derive(Debug, Clone)]
pub enum SandboxTransaction {
    CreateWorktreeAndAgent {
        worktree: Worktree,
        agent: Agent,
        event_at: String,
        event: SandboxEventKind,
    },
    SaveSandboxAndAppend {
        expected_revision: u64,
        sandbox: Sandbox,
        event_at: String,
        event: SandboxEventKind,
    },
}

#[async_trait]
pub trait SandboxTransactionStore: Send + Sync {
    async fn commit(&self, transaction: SandboxTransaction) -> Result<(), SandboxStoreError>;
}

#[derive(Clone, Default)]
pub struct InMemorySandboxStore {
    state: Arc<Mutex<State>>,
}

#[derive(Clone, Default)]
struct State {
    sandboxes: HashMap<SandboxId, Sandbox>,
    workspaces: HashMap<WorkspaceId, Workspace>,
    worktrees: HashMap<WorktreeId, Worktree>,
    agents: HashMap<AgentId, Agent>,
    sandbox_events: HashMap<SandboxId, Vec<SandboxEvent>>,
    agent_logs: HashMap<AgentId, Vec<AgentLogEntry>>,
}

impl InMemorySandboxStore {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn page<T: Clone>(
    mut values: Vec<(String, T)>,
    request: &PageRequest,
) -> Result<Page<T>, SandboxStoreError> {
    if request.limit == 0 || request.limit > MAX_PAGE_SIZE {
        return Err(SandboxStoreError::InvalidPage);
    }
    values.sort_by(|left, right| left.0.cmp(&right.0));
    if let Some(cursor) = &request.cursor {
        values.retain(|(key, _)| key > cursor);
    }
    let has_more = values.len() > usize::from(request.limit);
    values.truncate(usize::from(request.limit));
    let next = has_more
        .then(|| values.last().map(|(key, _)| key.clone()))
        .flatten();
    Ok(Page {
        items: values.into_iter().map(|(_, value)| value).collect(),
        next,
    })
}

mod memory;
