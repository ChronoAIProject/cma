use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::ids::{AgentId, SandboxId, SubjectId, WorkspaceId, WorktreeId};
mod sandbox_events;
pub use sandbox_events::SandboxEventStore;

use async_trait::async_trait;

use super::{
    Agent, AgentLogEntry, AgentLogKind, Sandbox, SandboxEvent, SandboxEventKind, SandboxStatus,
    Workspace, Worktree,
};

mod sandbox;
pub use sandbox::SandboxStore;
pub mod ledger_collection;
mod pagination;
mod service_traits;
pub use super::binding::BindingStore;
use pagination::page;
pub use service_traits::*;

mod agent;
pub use agent::AgentStore;
mod agent_log;
pub use agent_log::{AgentLogFinalAnswer, AgentLogStore};
#[cfg(any(test, feature = "test-support"))]
pub mod agent_log_contract;
mod agent_response_log;
pub use agent_response_log::{indexed, AgentResponseLogStore, AgentResponseLogWindow};
mod agent_log_publication;
pub use agent_log_publication::AgentLogPublicationIdentity;
mod sandbox_worker;
pub use sandbox_worker::SandboxWorkerStore;
mod work_authority;
mod work_lease;
mod work_snapshot;
pub use work_authority::{ClaimedWork, WorkAuthority};
pub use work_lease::{
    WorkAuthorityProtocol, WorkLeaseGrant, WORK_LEASE_SECONDS, WORK_RENEW_SECONDS,
};
pub use work_snapshot::{sandbox_content_matches, workspace_content_matches};
mod workspace_worker;
pub use workspace_worker::{WorkspacePreparationCompletion, WorkspaceWorkerStore};

mod agent_control;
mod agent_restart;
pub use agent_restart::{
    AgentRestartAcceptance, AgentRestartDispatch, AgentRestartRequest, AgentRestartSource,
    AgentRestartTeardown, AgentRestartTeardownPublication, AgentRestartWork,
};
mod background_checkpoint;
pub use agent_control::{
    AgentControlResult, AgentControlStore, AgentControlStoreError, AgentControlTarget,
    AgentDeleteRequest, AgentStopRequest,
};
mod agent_reads;
pub use agent_reads::AgentReadQueryStore;
mod reconciliation;
pub use reconciliation::{
    apply_sandbox_runtime_loss, precheck_runtime_loss_agents, precheck_sandbox_runtime_loss,
    retirement_debt_is_local, validate_sandbox_runtime_loss_request, SandboxReconciliationStore,
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
    ProvenanceBackfillOutcome, ProvenanceBackfillProgress,
};
pub use provenance_inventory::AgentProvenanceInventoryStore;
mod agent_admission;
pub use agent_admission::{
    AgentAdmission, AgentAdmissionResult, AgentAdmissionStore, AgentCreation,
};
mod agent_conversation;
mod agent_conversation_capacity;
pub use agent_conversation_capacity::AgentConversationCapacitySettlement;
mod stale_turn_settlement;
pub use stale_turn_settlement::StaleTurnSettlement;
mod agent_message_checkpoint;
mod agent_reopen;
pub use agent_conversation::{
    validate_agent_conversation_admission, AgentConversationMutation, AgentConversationStore,
};
pub use agent_message_checkpoint::{AgentMessageCheckpoint, AgentMessageSnapshot};
pub use agent_reopen::{AgentReopenGuard, OwnerRestartOutputFence, ReopenRefusalFence};
mod agent_lifecycle;
mod agent_skill_result;
pub use agent_skill_result::{requires_agent_skill_result_guard, AgentSkillResultGuard};
mod agent_log_capacity;
pub use agent_log_capacity::{plan_agent_log_capacity_failure, AGENT_LOG_CAPACITY_FAILURE};
mod agent_repeated_failure;
pub use agent_repeated_failure::{
    plan_agent_repeated_failure, validate_agent_repeated_failure, AgentRepeatedFailure,
};
mod agent_cleanup_capacity;
pub use agent_cleanup_capacity::{
    agent_cleanup_capacity_retry, agent_cleanup_capacity_retry_hint,
    plan_agent_cleanup_capacity_retry, plan_agent_marked_cleanup_capacity_retry,
    plan_agent_runtime_ledger_hold_retry, plan_agent_runtime_startup_hold_retry,
    validate_agent_cleanup_capacity_log, validate_agent_cleanup_capacity_log_with_retry,
    AGENT_RUNTIME_LEDGER_HOLD, AGENT_RUNTIME_LEDGER_HOLD_DETAIL, AGENT_RUNTIME_STARTUP_HOLD,
    AGENT_RUNTIME_STARTUP_HOLD_DETAIL, NORMAL_AGENT_LOG_ENTRIES,
};
mod agent_sign_in;
pub use agent_lifecycle::{
    apply_agent_connection_recovery, AgentConnectionRecoveryApplication,
    AgentConnectionRecoveryCandidate, AgentConnectionRecoveryCause, AgentConnectionRecoveryOutcome,
    AgentConnectionRecoveryRequest, AgentCreateAdmission, AgentRetryObservation,
    AgentWorkDiscovery, AgentWorkMutation, AgentWorkPublication, AgentWorkPublicationLineage,
    AgentWorkPublicationParents, AgentWorkerStore, PendingAgentWork, StartupWorkFilter,
    VersionedWorktree,
};
pub use agent_sign_in::apply_agent_sign_in;
mod agent_work_claim;
pub use agent_work_claim::{AgentWorkClaimDetail, AgentWorkClaimError};
mod agent_restart_ready;
pub use agent_restart_ready::owner_restart_ready;
mod agent_work_evidence_validation;
mod agent_work_prepared_open_ready;
mod agent_work_prepared_pair;
pub use agent_work_prepared_open_ready::{
    validate_prepared_agent_open_ready, PreparedAgentOpenContext,
};
mod agent_work_identity;
mod agent_work_transition_validation;
mod agent_work_validation;
mod agent_worktree_validation;
pub use agent_work_evidence_validation::{MAX_AGENT_CLEANUP_DEBT, MAX_AGENT_WORK_RECEIPTS};
pub use agent_work_prepared_pair::is_prepared_agent_replay_pair;
pub use agent_work_validation::{
    retired_sandbox_cleanup_is_absent, retiring_sandbox_process_is_absent,
    validate_agent_work_mutation, validate_agent_work_mutation_with_sandbox,
    validate_agent_workspace_binding,
};
mod sandbox_admission;
pub use sandbox_admission::{
    allocated_sandbox_name, valid_admission_fingerprint, CreationFence, SandboxAdmission,
    SandboxAdmissionName, SandboxAdmissionReplay, SandboxAdmissionResult, SandboxAdmissionStore,
    SandboxLimitClass,
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
mod custom_runtime;
pub use custom_runtime::*;
pub mod runtime_activity;
pub use runtime_activity::*;
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
    #[error("account deletion is in progress")]
    AccountDeletionInProgress,
    #[error(transparent)]
    Budget(crate::budget::BudgetError),
    #[error("new work is temporarily paused; existing work continues")]
    AdmissionPaused,
    #[error("active session capacity is unavailable")]
    SessionCapacityUnavailable,
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
    LimitReached {
        limit: u16,
        live: u16,
        usage: super::SandboxQuotaUsage,
    },
    #[error("global sandbox capacity is unavailable")]
    GlobalCapacityReached,
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
    #[error("workspace preparation recovery is not eligible")]
    WorkspaceRecoveryConflict,
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
    /// Snapshot-only callers keep the ordinary admission contract.
    async fn admit_catalogue_workspace(
        &self,
        admission: WorkspaceAdmission,
    ) -> Result<WorkspaceAdmissionResult, SandboxStoreError> {
        self.admit_workspace(admission).await
    }
    /// Explicitly recover only an original never-ready Workspace; all guards are atomic.
    async fn begin_workspace_prepare_recovery(
        &self,
        id: &WorkspaceId,
        caller: &crate::Caller,
        operation_id: &str,
        now: String,
    ) -> Result<Workspace, SandboxStoreError>;
    /// Fence new child admission and durably schedule removal, including child cleanup.
    async fn begin_workspace_removal(
        &self,
        id: &WorkspaceId,
        owner: &SubjectId,
        now: String,
    ) -> Result<Workspace, SandboxStoreError>;
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

#[derive(Debug, Clone)]
pub struct RepositoryPreparationRuntime {
    pub binding: super::RuntimeBinding,
    pub generation: u64,
    pub key_generation: u64,
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
        released_runtime: Option<layer3_adapter::ReleasedRuntime>,
        expected_revision: u64,
        sandbox: Box<Sandbox>,
        event_at: String,
        event: SandboxEventKind,
    },
    SaveWorkspaceAndAppend {
        /// Runtime that returned a terminal initial repository preparation failure.
        repository_runtime: Option<Box<RepositoryPreparationRuntime>>,
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
    #[cfg(test)]
    log_rows_read: Arc<std::sync::atomic::AtomicUsize>,
    #[cfg(test)]
    artifact_body_reads: Arc<std::sync::atomic::AtomicUsize>,
    managed_runtime_version: Option<String>,
    #[cfg(test)]
    managed_machine_runtime_version: Option<String>,
    state: Arc<Mutex<State>>,
    profiles: crate::profiles::InMemoryProfileStore,
}

#[derive(Clone, Default)]
struct State {
    admission_control: crate::admission::AdmissionControl,
    admission_audit: Vec<crate::ActivityRecord>,
    admission_history: Vec<crate::admission::AdmissionChange>,
    retained_reopen_receipts: HashMap<SandboxId, Vec<super::RetainedReopenReceipt>>,
    suggestions: HashMap<String, super::suggestions::SuggestionWork>,
    binding_association_index: std::collections::BTreeMap<(SubjectId, AgentId, String), ()>,
    binding_resets: HashMap<String, super::binding::ConversationResetState>,
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
    queued_input_edits: HashMap<crate::ids::AgentCommandId, super::agent_io::QueuedInputBinding>,
    queued_cancellations:
        HashMap<crate::ids::AgentCommandId, super::agent_io::QueuedCancellationBinding>,
    command_authorization_audits:
        std::collections::BTreeMap<String, super::agent_io::CommandAuthorizationAudit>,
    agent_command_keys: HashMap<(String, String), crate::ids::AgentCommandId>,
    agent_command_orders: HashMap<AgentId, super::agent_io::CommandLane>,
    #[cfg(test)]
    held_runtime_target: Option<String>,
    #[cfg(test)]
    expected_recovery_release: Option<layer3_adapter::ReleasedRuntime>,
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
    agent_retained_output: HashMap<AgentId, AgentMessageCheckpoint>,
    checkpoints: HashMap<(SandboxId, u64), super::SandboxCheckpointRef>,
    sandbox_sessions: std::collections::BTreeMap<(SandboxId, String), String>,
}

impl InMemorySandboxStore {
    #[cfg(test)]
    pub(crate) fn artifact_body_reads(&self) -> usize {
        self.artifact_body_reads
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    #[cfg(test)]
    pub(crate) fn log_rows_read(&self) -> usize {
        self.log_rows_read.load(std::sync::atomic::Ordering::SeqCst)
    }

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

mod admin_memory;
mod hierarchy_memory;
mod memory;
