//! The Sandbox -> Workspace -> Worktree -> Agent domain hierarchy.

mod admin;
mod agent;
pub mod agent_artifacts;
pub mod agent_attachments;
mod agent_context;
pub mod agent_io;
pub mod agent_live_skills;
mod agent_operation;
mod agent_settings;
pub use agent_live_skills::{AgentSkillInstallStatus, AgentSkillInstallation};
mod agent_settings_recovery;
mod agent_settings_validation;
pub use agent_settings::AgentSettings;
mod agent_operation_dispatch;
mod agent_operation_queue;
mod agent_operation_service;
mod agent_operation_settlement;
mod agent_read_types;
mod agent_reads;
mod agent_reads_fold;
mod agent_reads_usage;
mod agent_runtime;
mod agent_service;
mod agent_work;
mod background_checkpoint;
pub mod binding;
pub mod channel_usage;
mod checkpoint;
mod checkpoint_capture;
mod checkpoint_writer_recovery;
pub mod control;
mod conversation;
mod conversation_fold;
mod conversation_observation;
mod conversation_observation_fold;
pub use conversation_observation::{
    AgentObservations, AgentRuntimeMetadata, AgentTurnUsage, AgentUsageCheckpoint,
};
mod conversation_fold_support;
mod conversation_recorder;
mod creation_admission;
mod lifecycle_bootstrap;
mod provenance;
mod provenance_inventory;
mod provenance_service;
pub use provenance::{AgentCreationCategory, AgentCreationOrigin, AgentCreationProvenance};
pub use provenance_inventory::{
    AgentProvenanceInventoryQuery, AgentProvenanceInventoryRow, AgentProvenanceInventoryService,
};
pub use provenance_service::{
    AgentProvenanceDetails, AgentProvenanceReading, AgentProvenanceService,
};
mod binding_creation;
mod credential;
mod credential_prerequisite;
mod error;
mod event;
mod execution;
mod first_message;
mod first_message_dispatch;
mod first_message_service;
mod hierarchy;
mod home;
mod home_native;
pub use home_native::SandboxNativePool;
mod home_recovery;
mod inactivity;
mod launch_recovery;
mod launch_service;
mod lifecycle;
mod lifecycle_execution;
mod lifecycle_execution_support;
mod lifecycle_service;
mod lifecycle_service_access;
mod limits;
mod pending_approvals;
mod provision;
mod queued_message;
mod revive;
mod rotate;
mod runtime_generation;
mod runtime_health;
mod runtime_recovery;
mod runtime_recovery_agents;
#[allow(clippy::module_inception)]
mod sandbox;
mod service;
mod service_access;
mod service_access_service;
#[cfg(test)]
mod service_access_tests;
mod service_admission;
mod service_creation;
mod service_scope;
#[cfg(test)]
mod service_scope_tests;
mod service_trust;
mod status;
pub mod store;
mod title;
mod title_sanitize;
mod title_service;
mod title_store;
mod warmup;
mod workspace;
mod workspace_service;
mod workspace_work;
mod worktree;
pub use home_recovery::{SandboxHomeRecovery, SandboxRecoveryPause};
pub use warmup::{
    WarmAgent, WarmSandbox, WarmSandboxPlan, WarmSandboxState, WarmWorkspace, WarmWorkspacePlan,
};

pub use admin::{
    AgentAdminSummary, OperatorError, SandboxAdminQuery, SandboxAdminStore, SandboxAdminSummary,
    SandboxAdminView, SandboxOperatorService, SandboxRetirement, SandboxRetirementResult,
    StoredSandboxRetirement, WorkspaceAdminSummary,
};
pub use agent::{Agent, AgentOrigin, AgentStopReason, ConversationState};
pub use agent_context::{
    AgentContextDefaults, AgentContextError, AgentContextOverrides, AgentContextService,
    AGENT_CONTEXT_ADMISSION_BOUND,
};
pub use agent_operation::{
    AgentApprovalDecision, AgentOperationDelivery, AgentOperationFailure, AgentOperationIntent,
    AgentOperationKind, AgentOperationRecord,
};
pub use agent_operation_service::{
    AgentMessageOutcome, AgentOperationError, AgentOperationReceipt, AgentOperationService,
    AgentOperationStore,
};
pub use agent_read_types::{
    AgentLineage, AgentListQuery, AgentReadError, AgentResourceReading, AgentTranscript,
    AgentTranscriptMessage, AgentTranscriptRole, AgentTranscriptTurn, AgentTurnOutcome, AgentUsage,
    AgentUsageReading, SandboxResourceReadError, SandboxResourceReader, SandboxResourceReading,
};
pub use agent_reads::{
    AgentDetailReading, AgentEventStreamAuthorization, AgentEventStreamHeader,
    AgentEventStreamSnapshot, AgentReadService, AgentReplayEntry, AgentReplayWindow,
    AgentResponseBinding, AgentSourceReadError, AgentSourceReading, AgentVisibleLogEntry,
    SharedAgentDetail, SharedAgentOrigin,
};
pub use agent_runtime::{
    AgentRuntimeConnection, AgentRuntimeError, AgentRuntimeInputError, AgentRuntimeInputSender,
    AgentRuntimePromptResponse, AgentRuntimeRegistry, AgentRuntimeTarget,
    AgentRuntimeTurnStopReason, AgentRuntimeUpdate,
};
pub use agent_service::{AgentService, AgentServiceError, CreateAgent};
pub use agent_work::{
    AgentCleanupDebt, AgentWorkLease, AgentWorkReceipt, AgentWorkState, AgentWorkStep,
    ResolvedAgentSkill,
};
pub use background_checkpoint::{
    SandboxBackgroundCheckpoint, SandboxBackgroundCheckpointPlan,
    SandboxBackgroundCheckpointProgress,
};
pub use checkpoint::{
    sandbox_checkpoint_due, sandbox_checkpoint_due_with_interval, SandboxCheckpointConversation,
    SandboxCheckpointConversationRef, SandboxCheckpointHistory, SandboxCheckpointPublication,
    SandboxCheckpointRef, MAX_SANDBOX_CONVERSATION_SNAPSHOT_BYTES,
    SANDBOX_CHECKPOINT_INTERVAL_SECONDS,
};
pub use checkpoint_capture::{SandboxCheckpointCapture, SandboxCheckpointCapturePhase};
pub use conversation::{
    AgentConversationError, AgentMessageAssembly, AgentUpdateOutcome, AgentUpdateRecorder,
    MAX_DURABLE_AGENT_MESSAGE_BYTES,
};
pub use conversation_recorder::{
    AgentLiveFragment, AgentLiveFragmentKind, AgentLiveSubscription, DurableAgentUpdateRecorder,
};
pub use credential::{
    SandboxCredentialAuthority, SandboxCredentialSource, SandboxCredentialState, SandboxRevokeDebt,
    SandboxServiceGrant, SandboxServiceScopeRevision, SandboxSignInState,
};
pub use credential_prerequisite::{
    FixedSandboxCredentialPrerequisite, SandboxCredentialPrerequisite,
    SandboxCredentialPrerequisiteError,
};
pub use error::SandboxModelError;
pub use event::{
    AgentAvailableCommand, AgentConfigOption, AgentLogEntry, AgentLogKind, AgentPlanStep,
    SandboxEvent, SandboxEventKind,
};
pub use execution::ProvisioningExecution;
pub use first_message::{
    FirstMessageDelivery, FirstMessageFailure, FirstMessageRecord, MAX_FIRST_MESSAGE_BYTES,
};
pub use first_message_service::{AgentWorkAuthority, FirstMessageDispatchOutcome};
pub use hierarchy::{
    AgentHierarchySummary, HierarchyCounts, HierarchyReadError, HierarchyReadService,
    HierarchyReadStore, OwnerSandboxQuery, SandboxHierarchyReading, SandboxHierarchyRecord,
    SandboxHierarchySummary, SandboxHierarchyView, SandboxInventoryReading, SandboxInventoryRecord,
    SandboxSnapshotDownload, SharedHierarchyAgent, SharedSandboxHierarchy, SharedSandboxSummary,
    SharedWorkspaceHierarchy, SharedWorkspaceSummary, SharedWorktreeSummary,
    WorkspaceHierarchyReading, WorkspaceHierarchyRecord, WorkspaceHierarchySummary,
    WorkspaceHierarchyView, WorktreeHierarchyReading, WorktreeHierarchySummary,
};
pub use home::SandboxHome;
pub use inactivity::{
    inactivity_pause_notice, is_inactivity_pause_candidate, SandboxAgentActivity,
    SandboxAgentLogActivity,
};
pub use lifecycle::{
    SandboxImageRotationPhase, SandboxLifecycleAuthority, SandboxLifecycleOperation,
    SandboxLifecycleState, SandboxPausePhase,
};
pub use lifecycle_service::SandboxLifecycleCommandError;
pub use limits::{enforce_agent_limit, enforce_workspace_limit, SandboxLimitError};
pub use pending_approvals::PendingAgentApprovals;
pub use provision::{SandboxWorkLease, SandboxWorkState, SandboxWorkStep};
pub use queued_message::{
    QueuedAgentMessage, QueuedMessageState, MAX_AGENT_MESSAGE_BYTES, MAX_QUEUED_AGENT_MESSAGES,
    QUEUED_MESSAGE_DISPATCH_GRACE_SECONDS,
};
pub use revive::{
    SandboxReviveCause, SandboxReviveFailureClass, SandboxRevivePhase, SANDBOX_REVIVE_PHASE_ORDER,
};
pub use rotate::{
    image_rotation_required, SandboxRotationAccounting, SandboxRotationAccountingOutcome,
    SandboxRotationAgentIdentity, SandboxRotationIdentity,
};
pub use runtime_generation::SandboxRuntimeProvisionReservation;
pub use sandbox::{RuntimeBinding, Sandbox, SandboxLifecycleBlocker, SandboxLimits, TypedFailure};
pub use service::{
    CreateSandbox, SandboxOrigin, SandboxPolicy, SandboxPolicySource, SandboxService,
    SandboxServiceError,
};
pub(crate) use service_trust::foreign_workspace_setups;
pub use status::{AgentStatus, SandboxStatus, WorkspaceStatus, WorktreeStatus};
pub use store::{
    InMemorySandboxStore, Page, PageRequest, SandboxImageRotationRequest,
    SandboxInactivityPauseRequest, SandboxLaunchCommandDisposition, SandboxLaunchCommandOutcome,
    SandboxLifecycleCommandDisposition, SandboxLifecycleCommandOutcome, SandboxMaintenanceOutcome,
    SandboxMaintenanceStore, SandboxStoreError,
};
pub use title::{
    AgentTitle, AgentTitleError, AgentTitleGenerationContext, AgentTitleGenerationError,
    AgentTitleGenerationRequest, AgentTitleGenerationResponse, AgentTitleGenerator,
    AgentTitleSource, AgentTitleTranscript, AGENT_TITLE_ATTEMPTS, AGENT_TITLE_CALL_TIMEOUT_SECONDS,
    AGENT_TITLE_CLAIM_SECONDS, AGENT_TITLE_FALLBACK_SCALARS, AGENT_TITLE_INPUT_BYTES,
    AGENT_TITLE_MAX_SCALARS, AGENT_TITLE_SYSTEM_PROMPT, EMPTY_AGENT_TITLE_FALLBACK,
};
pub use title_service::{AgentTitleOutcome, AgentTitleService, AgentTitleServiceError};
pub use title_store::{
    AgentTitleClaim, AgentTitleClaimMode, AgentTitleClaimRequest, AgentTitleStore,
    AgentTitleStoreError, InMemoryAgentTitleStore, SetAgentTitleOutcome, StoredAgentTitle,
};
pub use workspace::{Workspace, WorkspaceOrigin, WorkspaceSource};
pub use workspace_service::{
    CreateInlineWorkspace, CreateWorkspace, InlineWorkspaceSource, WorkspaceService,
    WorkspaceServiceError,
};
pub use workspace_work::{WorkspaceWorkLease, WorkspaceWorkState, WorkspaceWorkStep};
pub use worktree::Worktree;

#[cfg(test)]
mod agent_model_tests;
#[cfg(test)]
mod agent_operation_tests;
#[cfg(test)]
mod agent_reads_tests;
#[cfg(test)]
mod agent_service_tests;
#[cfg(test)]
mod checkpoint_tests;
#[cfg(test)]
mod hierarchy_tests;
#[cfg(test)]
mod inactivity_tests;
#[cfg(test)]
mod launch_service_tests;
#[cfg(test)]
mod lifecycle_eligibility_tests;
#[cfg(test)]
mod lifecycle_execution_tests;
#[cfg(test)]
mod lifecycle_service_tests;
#[cfg(test)]
mod lifecycle_test_support;
#[cfg(test)]
mod lifecycle_tests;
#[cfg(test)]
mod revive_tests;
#[cfg(test)]
mod rotate_tests;
#[cfg(test)]
mod runtime_generation_tests;
#[cfg(test)]
mod service_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod title_service_tests;
#[cfg(test)]
mod title_store_edge_tests;
#[cfg(test)]
mod title_store_tests;
#[cfg(test)]
mod title_value_tests;
#[cfg(test)]
mod workspace_service_tests;

pub mod agent_access;
pub mod agent_access_tool;
mod suspended_runtime;
