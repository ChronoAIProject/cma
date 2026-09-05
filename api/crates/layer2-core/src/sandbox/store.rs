use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use crate::ids::{AgentId, SandboxId, SubjectId, WorkspaceId, WorktreeId};

use super::{
    Agent, AgentLogEntry, AgentLogKind, Sandbox, SandboxEvent, SandboxEventKind, SandboxStatus,
    Workspace, Worktree,
};

mod agent_control;
pub use agent_control::{
    AgentControlResult, AgentControlStore, AgentControlStoreError, AgentControlTarget,
    AgentDeleteRequest, AgentStopRequest,
};
mod agent_reads;
pub use agent_reads::AgentReadQueryStore;
mod agent_admission;
pub use agent_admission::{
    AgentAdmission, AgentAdmissionResult, AgentAdmissionStore, AgentCreation,
};
mod agent_conversation;
pub use agent_conversation::{AgentConversationMutation, AgentConversationStore};
mod agent_lifecycle;
pub use agent_lifecycle::{AgentWorkMutation, AgentWorkerStore, VersionedWorktree};
mod agent_work_evidence_validation;
mod agent_work_transition_validation;
mod agent_work_validation;
pub use agent_work_evidence_validation::{MAX_AGENT_CLEANUP_DEBT, MAX_AGENT_WORK_RECEIPTS};
pub use agent_work_validation::validate_agent_work_mutation;
#[cfg(test)]
mod agent_work_validation_tests;

pub const MAX_PAGE_SIZE: u16 = 200;
pub const MAX_AGENT_LOG_BATCH_SIZE: usize = 64;

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
    #[error("agent log capacity exceeded")]
    AgentLogCapacityExceeded,
    #[error("sandbox limit reached: {live} of {limit}")]
    LimitReached { limit: u16, live: u16 },
    #[error("sandbox is not ready for workspace admission")]
    SandboxNotReady,
    #[error("workspace name is already live in the sandbox")]
    WorkspaceNameConflict,
    #[error("workspace limit reached: {live} of {limit}")]
    WorkspaceLimitReached { limit: u8, live: u8 },
    #[error("workspace still has live agents")]
    WorkspaceHasLiveAgents,
    #[error("workspace provisioning origin is already occupied")]
    WorkspaceOriginConflict,
    #[error("workspace is not ready for agent admission")]
    WorkspaceNotReady,
    #[error("agent limit reached: {live} of {limit}")]
    AgentLimitReached { limit: u8, live: u8 },
    #[error("agent creation key is already bound to another request")]
    AgentCreationConflict,
    #[error("invalid page request")]
    InvalidPage,
    #[error("transaction invariant failed: {0}")]
    InvalidTransaction(&'static str),
    #[error("sandbox store unavailable: {0}")]
    Unavailable(String),
    #[error("stored sandbox data is not usable: {0}")]
    Corrupt(String),
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceAdmissionKind {
    Manual,
    SandboxProfile,
}

impl WorkspaceAdmissionKind {
    pub const fn allowed_status(self) -> SandboxStatus {
        match self {
            Self::Manual => SandboxStatus::Active,
            Self::SandboxProfile => SandboxStatus::Installing,
        }
    }
}

#[derive(Debug, Clone)]
pub struct WorkspaceAdmission {
    pub workspace: Workspace,
    pub kind: WorkspaceAdmissionKind,
    pub event_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceAdmissionResult {
    Created(Workspace),
    Existing(Workspace),
}

/// Atomic sandbox readiness, lineage/name/limit admission and Created event.
#[async_trait]
pub trait WorkspaceAdmissionStore: Send + Sync {
    async fn admit_workspace(
        &self,
        admission: WorkspaceAdmission,
    ) -> Result<WorkspaceAdmissionResult, SandboxStoreError>;

    /// Lock and rewrite the workspace row while checking that no agent is live.
    async fn begin_workspace_removal(
        &self,
        id: &WorkspaceId,
        owner: &SubjectId,
        now: String,
    ) -> Result<Workspace, SandboxStoreError>;
}

/// Durable workspace lifecycle discovery and lease fencing.
#[async_trait]
pub trait WorkspaceWorkerStore: Send + Sync {
    async fn due_workspace_work(
        &self,
        now: &str,
        limit: u16,
    ) -> Result<Vec<WorkspaceId>, SandboxStoreError>;
    async fn claim_workspace_work(
        &self,
        id: &WorkspaceId,
        holder: &str,
        now: String,
        lease_expires_at: String,
    ) -> Result<Option<Workspace>, SandboxStoreError>;
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SandboxAdmissionName {
    Exact(String),
    Allocate(String),
}

#[derive(Debug, Clone)]
pub struct SandboxAdmission {
    pub sandbox: Sandbox,
    pub name: SandboxAdmissionName,
    pub live_limit: u16,
    pub event_at: String,
}

/// Atomic live-limit admission, name allocation, aggregate insert and Created event.
#[async_trait]
pub trait SandboxAdmissionStore: Send + Sync {
    async fn admit_sandbox(
        &self,
        admission: SandboxAdmission,
    ) -> Result<Sandbox, SandboxStoreError>;
}

/// Durable lifecycle work discovery, lease fencing and terminal purge.
#[async_trait]
pub trait SandboxWorkerStore: Send + Sync {
    async fn due_sandbox_work(
        &self,
        now: &str,
        limit: u16,
    ) -> Result<Vec<SandboxId>, SandboxStoreError>;
    async fn claim_sandbox_work(
        &self,
        id: &SandboxId,
        holder: &str,
        now: String,
        lease_expires_at: String,
    ) -> Result<Option<Sandbox>, SandboxStoreError>;
    async fn purge_sandbox(
        &self,
        id: &SandboxId,
        expected_revision: u64,
        now: &str,
    ) -> Result<(), SandboxStoreError>;
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
        worktree: Box<Worktree>,
        agent: Box<Agent>,
        event_at: String,
        event: SandboxEventKind,
    },
    SaveSandboxAndAppend {
        expected_revision: u64,
        sandbox: Box<Sandbox>,
        event_at: String,
        event: SandboxEventKind,
    },
    SaveWorkspaceAndAppend {
        expected_revision: u64,
        workspace: Box<Workspace>,
        event_at: String,
        event: SandboxEventKind,
    },
}

#[async_trait]
pub trait SandboxTransactionStore: Send + Sync {
    async fn commit(&self, transaction: SandboxTransaction) -> Result<(), SandboxStoreError>;
}

pub trait SandboxServiceStore:
    SandboxStore + SandboxEventStore + SandboxAdmissionStore + SandboxTransactionStore
{
}

impl<T> SandboxServiceStore for T where
    T: SandboxStore + SandboxEventStore + SandboxAdmissionStore + SandboxTransactionStore
{
}

pub trait SandboxLifecycleStore:
    SandboxStore + SandboxEventStore + SandboxTransactionStore + SandboxWorkerStore
{
}

impl<T> SandboxLifecycleStore for T where
    T: SandboxStore + SandboxEventStore + SandboxTransactionStore + SandboxWorkerStore
{
}

pub trait WorkspaceServiceStore:
    SandboxStore + WorkspaceStore + WorkspaceAdmissionStore + SandboxEventStore
{
}

impl<T> WorkspaceServiceStore for T where
    T: SandboxStore + WorkspaceStore + WorkspaceAdmissionStore + SandboxEventStore
{
}

pub trait WorkspaceLifecycleStore:
    SandboxStore + WorkspaceStore + WorkspaceWorkerStore + SandboxTransactionStore
{
}

pub trait AgentServiceStore:
    SandboxStore
    + WorkspaceStore
    + WorktreeStore
    + AgentStore
    + AgentAdmissionStore
    + AgentConversationStore
    + AgentControlStore
    + AgentLogStore
{
}

pub trait AgentLifecycleStore:
    SandboxStore + WorkspaceStore + WorktreeStore + AgentStore + AgentLogStore + AgentWorkerStore
{
}

pub trait AgentReadStore:
    SandboxStore + WorkspaceStore + WorktreeStore + AgentStore + AgentLogStore + AgentReadQueryStore
{
}

pub trait AgentConversationUpdateStore: AgentStore + AgentConversationStore {}

impl<T> AgentConversationUpdateStore for T where T: AgentStore + AgentConversationStore {}

impl<T> AgentLifecycleStore for T where
    T: SandboxStore
        + WorkspaceStore
        + WorktreeStore
        + AgentStore
        + AgentLogStore
        + AgentWorkerStore
{
}

impl<T> AgentReadStore for T where
    T: SandboxStore
        + WorkspaceStore
        + WorktreeStore
        + AgentStore
        + AgentLogStore
        + AgentReadQueryStore
{
}

impl<T> AgentServiceStore for T where
    T: SandboxStore
        + WorkspaceStore
        + WorktreeStore
        + AgentStore
        + AgentAdmissionStore
        + AgentConversationStore
        + AgentControlStore
        + AgentLogStore
{
}

impl<T> WorkspaceLifecycleStore for T where
    T: SandboxStore + WorkspaceStore + WorkspaceWorkerStore + SandboxTransactionStore
{
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

mod admin_memory;
mod hierarchy_memory;
mod memory;
