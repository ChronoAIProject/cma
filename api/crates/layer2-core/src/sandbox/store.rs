use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use crate::ids::{AgentId, SandboxId, SubjectId, WorkspaceId, WorktreeId};

use super::{
    Agent, AgentLogEntry, AgentLogKind, Sandbox, SandboxEvent, SandboxEventKind, SandboxStatus,
    Workspace, Worktree,
};

mod service_traits;
pub use super::binding::BindingStore;
pub use service_traits::*;

mod agent;
pub use agent::AgentStore;

mod agent_control;
mod background_checkpoint;
pub use agent_control::{
    AgentControlResult, AgentControlStore, AgentControlStoreError, AgentControlTarget,
    AgentDeleteRequest, AgentStopRequest,
};
mod agent_reads;
pub use agent_reads::AgentReadQueryStore;
mod reconciliation;
pub use reconciliation::{
    apply_sandbox_runtime_loss, validate_sandbox_runtime_loss_request, SandboxReconciliationStore,
    SandboxRuntimeLossAgentMutation, SandboxRuntimeLossApplication, SandboxRuntimeLossMutation,
    SandboxRuntimeLossOutcome, SandboxRuntimeLossRequest,
};
mod channel_usage;
pub use channel_usage::{
    channel_usage_cursor, channel_usage_page, AgentChannelUsageSnapshot, AgentChannelUsageStore,
};
mod provenance;
mod provenance_inventory;
pub use provenance::{
    AgentProvenanceBackfillStore, AgentProvenanceSnapshot, AgentProvenanceStore,
    ProvenanceBackfillOutcome,
};
pub use provenance_inventory::AgentProvenanceInventoryStore;
mod agent_admission;
pub use agent_admission::{
    AgentAdmission, AgentAdmissionResult, AgentAdmissionStore, AgentCreation,
};
mod agent_conversation;
mod agent_message_checkpoint;
pub use agent_conversation::{
    validate_agent_conversation_admission, AgentConversationMutation, AgentConversationStore,
};
pub use agent_message_checkpoint::AgentMessageCheckpoint;
mod agent_lifecycle;
mod agent_sign_in;
pub use agent_lifecycle::{
    apply_agent_connection_recovery, AgentConnectionRecoveryApplication,
    AgentConnectionRecoveryCandidate, AgentConnectionRecoveryCause, AgentConnectionRecoveryOutcome,
    AgentConnectionRecoveryRequest, AgentWorkMutation, AgentWorkerStore, VersionedWorktree,
};
pub use agent_sign_in::apply_agent_sign_in;
mod agent_work_evidence_validation;
mod agent_work_transition_validation;
mod agent_work_validation;
mod agent_worktree_validation;
pub use agent_work_evidence_validation::{MAX_AGENT_CLEANUP_DEBT, MAX_AGENT_WORK_RECEIPTS};
pub use agent_work_validation::{
    retired_sandbox_cleanup_is_absent, retiring_sandbox_process_is_absent,
    validate_agent_work_mutation, validate_agent_work_mutation_with_sandbox,
    validate_agent_workspace_binding,
};
mod sandbox_admission;
pub use sandbox_admission::{
    allocated_sandbox_name, valid_admission_fingerprint, SandboxAdmission, SandboxAdmissionName,
    SandboxAdmissionReplay, SandboxAdmissionResult, SandboxAdmissionStore,
};
mod lifecycle_command;
pub use lifecycle_command::*;
mod command_revival;
pub use command_revival::{validate_command_revival_transition, CommandRevivalStore};
mod launch_command;
pub use launch_command::*;
pub mod checkpoint_capture;
mod lifecycle_execution;
pub use lifecycle_execution::*;
mod maintenance;
pub use maintenance::*;
mod quiet_pause;
pub use quiet_pause::apply_consented_quiet_pause;
#[cfg(test)]
mod agent_work_validation_tests;
mod lifecycle_agent_validation;
mod lifecycle_event_validation;
mod lifecycle_execution_validation;
mod lifecycle_queue_recovery_validation;

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
    #[error("sandbox checkpoint is in progress; input was not admitted")]
    CheckpointInProgress,
    #[error("sandbox lifecycle is busy: {status:?} ({reason:?})")]
    LifecycleBusy {
        status: SandboxStatus,
        reason: super::SandboxLifecycleBlocker,
    },
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
    #[error("sandbox trigger origin is already bound to another request")]
    SandboxOriginConflict,
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
    Trigger,
    Lifecycle,
}

impl WorkspaceAdmissionKind {
    pub const fn allowed_status(self) -> SandboxStatus {
        match self {
            Self::Manual | Self::Trigger | Self::Lifecycle => SandboxStatus::Active,
            Self::SandboxProfile => SandboxStatus::Installing,
        }
    }
}

#[derive(Debug, Clone)]
pub struct WorkspaceAdmission {
    pub binding: Option<super::binding::BindingAdmissionFence>,
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

    /// Fence new child admission and durably schedule removal, including child cleanup.
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

    /// Read the exact prefix needed for a captured Agent projection.
    async fn agent_log_prefix(
        &self,
        agent_id: &AgentId,
        through_seq: u64,
    ) -> Result<Vec<AgentLogEntry>, SandboxStoreError> {
        let mut entries = Vec::new();
        let mut after = 0;
        while after < through_seq {
            let limit = (through_seq - after).min(u64::from(MAX_PAGE_SIZE)) as u16;
            let page = self.agent_log(agent_id, after, limit).await?;
            if page.len() != usize::from(limit) {
                return Err(SandboxStoreError::SequenceConflict);
            }
            after += u64::from(limit);
            entries.extend(page);
        }
        Ok(entries)
    }
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
        /// Internal authority for deferred command revival; absent on legacy
        /// owner/system lifecycle transactions. Never serialized into aggregates.
        command_revival: Option<Arc<super::agent_io::CommandRevivalAuthorization>>,
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

#[derive(Clone, Default)]
pub struct InMemorySandboxStore {
    state: Arc<Mutex<State>>,
    profiles: crate::profiles::InMemoryProfileStore,
}

#[derive(Clone, Default)]
struct State {
    binding_key_history: std::collections::HashSet<String>,
    bindings: std::collections::BTreeMap<String, super::binding::ProviderBinding>,
    binding_conversations: std::collections::BTreeMap<String, super::binding::BindingConversation>,
    agent_artifacts: HashMap<String, super::agent_artifacts::StoredAgentArtifact>,
    agent_channel_usage: HashMap<AgentId, super::channel_usage::StoredAgentChannelUsage>,
    channel_usage_events:
        std::collections::BTreeMap<(AgentId, u64), super::channel_usage::ChannelUsageEvent>,
    channel_usage_operations: HashMap<String, (AgentId, u64)>,
    channel_usage_bindings: HashMap<String, (AgentId, u64)>,
    agent_creation_provenance: HashMap<AgentId, super::AgentCreationProvenance>,
    resource_policies: HashMap<SandboxId, crate::resource_access::ResourcePolicyRecord>,
    resource_shares: HashMap<String, crate::resource_access::ShareMutationReceipt>,
    resource_share_commands: HashMap<String, crate::resource_access::ShareMutationReceipt>,
    sandbox_controls: HashMap<crate::ids::SandboxCommandId, super::control::SandboxControlRecord>,
    sandbox_control_keys: HashMap<(String, String), crate::ids::SandboxCommandId>,
    agent_commands: HashMap<crate::ids::AgentCommandId, super::agent_io::AdmittedAgentCommand>,
    targeted_controls: HashMap<crate::ids::AgentCommandId, super::agent_io::TargetedControlBinding>,
    queued_cancellations:
        HashMap<crate::ids::AgentCommandId, super::agent_io::QueuedCancellationBinding>,
    command_authorization_audits:
        std::collections::BTreeMap<String, super::agent_io::CommandAuthorizationAudit>,
    agent_command_keys: HashMap<(String, String), crate::ids::AgentCommandId>,
    agent_command_orders: HashMap<AgentId, super::agent_io::CommandLane>,
    sandboxes: HashMap<SandboxId, Sandbox>,
    workspaces: HashMap<WorkspaceId, Workspace>,
    worktrees: HashMap<WorktreeId, Worktree>,
    agents: HashMap<AgentId, Agent>,
    agent_reuse_bindings: HashMap<(SubjectId, String), (AgentId, String)>,
    sandbox_admissions:
        HashMap<(SubjectId, String, String), (SandboxId, SandboxAdmissionName, String)>,
    sandbox_events: HashMap<SandboxId, Vec<SandboxEvent>>,
    agent_logs: HashMap<AgentId, Vec<AgentLogEntry>>,
    agent_message_checkpoints: HashMap<AgentId, AgentMessageCheckpoint>,
    checkpoints: HashMap<(SandboxId, u64), super::SandboxCheckpointRef>,
}

impl InMemorySandboxStore {
    pub fn with_profiles(profiles: crate::profiles::InMemoryProfileStore) -> Self {
        Self {
            profiles,
            ..Self::default()
        }
    }

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
