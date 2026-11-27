//! The Sandbox -> Workspace -> Worktree -> Agent domain hierarchy.
mod desktop;
mod runtime_migration_guard;
pub use desktop::{
    DesktopAdmission, DesktopAuthorization, DesktopConnection, DesktopError, DesktopFrame,
    DesktopService,
};
pub use runtime_migration_guard::{
    runtime_desktop_activity_snapshot, runtime_desktop_process_incarnation,
    try_acquire_runtime_migration_guard, RuntimeDesktopActivitySnapshot, RuntimeMigrationBusy,
    RuntimeMigrationGuard,
};
mod admin;
mod auto_pause_operator;
pub use auto_pause_operator::{
    AutoPauseBatch, AutoPauseSandboxRefusal, SandboxAutoPauseService, AUTO_PAUSE_BATCH_LIMIT,
    AUTO_PAUSE_MAX_BATCH,
};
mod agent;
pub mod agent_access;
pub mod agent_access_tool;
pub mod agent_artifacts;
pub mod agent_attachments;
pub mod agent_browser_control;
pub mod agent_capacity_recovery;
mod agent_context;
pub mod agent_environment;
pub mod agent_filesystem;
pub mod agent_io;
pub mod agent_live_skills;
mod agent_managed_context;
mod agent_operation;
pub mod agent_output_index;
mod agent_preferences;
mod agent_settings;
pub mod desktop_creation;
pub mod task_acceptance;
#[cfg(test)]
mod task_acceptance_tests;
pub mod task_experience;
pub mod task_template;
pub use agent_preferences::{AgentInstructionPolicy, AgentPreferences, AgentPreferencesUpdate};
mod agent_settings_authority;
pub use agent_live_skills::{AgentSkillInstallStatus, AgentSkillInstallation};
pub use agent_settings_authority::{AgentSettingsAuthorization, AgentSettingsFence};
mod agent_settings_recovery;
mod agent_settings_validation;
pub use agent_settings::AgentSettings;
mod admission;
mod admission_checkpoint;
mod agent_failure_projection;
pub mod agent_observation;
mod agent_operation_dispatch;
mod agent_operation_queue;
mod agent_operation_service;
mod agent_operation_settlement;
mod agent_read_types;
mod agent_reply_preview;
pub use agent_failure_projection::{public_agent_turn_failure, sanitize_agent_transcript};
pub use agent_reply_preview::{normalize_reply_preview, AgentReplyPreview, AssistantReplyPreview};
mod agent_reads;
pub use admission_checkpoint::AgentLogCheckpoint;
mod agent_log_projection;
mod agent_reads_fold;
mod agent_reads_transcript;
mod agent_reads_usage;
mod agent_transcript_window;
pub use agent_log_projection::{
    AgentLogAnswerEvidence, AgentLogAnswerLookup, AgentLogEvidenceChange, AgentLogProjectionState,
    AgentLogQueuedReference, AgentLogTurnFacts, AgentLogTurnLookup, BoundedAgentLogFold,
};
pub use agent_reads_transcript::{
    AgentTranscriptFold, AgentTranscriptPage, AgentTranscriptPageRequest,
};
pub use agent_transcript_window::AgentTranscriptWindow;
mod agent_runtime;
mod agent_service;
mod agent_work;
mod agent_worktree_recovery;
pub mod answer_deliverables;
mod owner_agent_restart;
pub use agent_service::{AgentRestartControl, AgentRestartObservation, AgentRestartService};
pub mod work_retry;
pub use agent_worktree_recovery::{
    plan_agent_worktree_recovery, AgentWorktreeRecovery, AgentWorktreeRecoveryContext,
    AgentWorktreeRecoveryMutation,
};
mod background_checkpoint;
pub mod binding;
pub mod channel_usage;
mod checkpoint;
mod checkpoint_capture;
mod checkpoint_writer_recovery;
pub mod colors;
pub mod control;
mod conversation;
mod conversation_fold;
mod conversation_observation;
mod conversation_observation_fold;
pub mod ingress_ownership;
pub mod message_feedback;
pub mod provider_associations;
pub mod suggestions;
pub use conversation_observation::{
    AgentDescriptionSource, AgentObservations, AgentRuntimeMetadata, AgentTurnUsage,
    AgentUsageCheckpoint,
};
pub mod agent_ledger_recovery;
mod conversation_fold_support;
mod conversation_record_observation;
mod conversation_recorder;
pub use conversation_record_observation::{observe_agent_output, AgentOutputCategory};
mod creation_admission;
pub mod ledger_census;
pub mod ledger_collection_artifact;
pub mod ledger_maintenance;
mod ledger_retirement;
mod lifecycle_bootstrap;
mod provenance;
mod provenance_inventory;
mod provenance_service;
pub mod retirement_archive;
pub mod reviewed_retirement;
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
mod failed_provision;
mod first_message;
mod first_message_dispatch;
mod first_message_service;
pub use failed_provision::SandboxFailedRuntimeProvision;
mod hierarchy;
pub mod task_activity;
mod task_status;
pub use task_status::AgentTaskStatus;
mod home;
pub mod home_backup;
mod home_native;
pub use home_native::SandboxNativePool;
mod home_recovery;
mod home_restore;
pub use home_restore::{
    SandboxHomeRestore, SandboxHomeRestoreAttestation, SandboxHomeRestoreEvidence,
    SandboxHomeRestoreEvidenceKind, SandboxHomeRestoreReceipt, SandboxSupersededHome,
};
mod idle_evidence;
mod inactivity;
pub use idle_evidence::{agentless_idle_refusal, agentless_idle_with_children};
mod interruption;
mod launch_recovery;
mod launch_service;
mod lifecycle;
mod lifecycle_result;
pub use lifecycle_result::{
    SandboxLifecycleAction, SandboxLifecycleAdmission, SandboxLifecycleResult,
    SandboxLifecycleResultState, LIFECYCLE_RESULT_SECONDS,
};
mod lifecycle_execution;
mod lifecycle_execution_support;
mod lifecycle_service;
mod lifecycle_service_access;
mod limits;
mod pending_approvals;
mod provision;
mod queued_message;
mod restored_connection;
mod retained_recovery;
mod retained_reopen;
mod retained_reopen_service;
#[cfg(test)]
mod retained_reopen_tests;
mod retained_suspended_reopen;
pub mod turn_timeout;
pub use retained_reopen::{
    apply_retained_reopen, RetainedReopenAccepted, RetainedReopenHistory, RetainedReopenProof,
    RetainedReopenReceipt, RetainedReopenReplay, RetainedReopenRequest, RetainedReopenSnapshot,
    RetainedReopenVerifier,
};
pub use retained_reopen_service::{RetainedReopenError, RetainedReopenPath, RetainedReopenService};
pub use retained_suspended_reopen::{
    apply_suspended_reopen, RetainedSuspendedReopenProof, RetainedSuspendedReopenReceipt,
    RetainedSuspendedReopenRequest,
};
mod revive;
mod rotate;
mod runtime_generation;
mod runtime_health;
mod runtime_recovery;
pub use retained_recovery::{
    RetainedRecoveryAssessment, RetainedRecoveryDisposition, RetainedRecoveryError,
    RetainedRecoveryInspector, RetainedRecoveryService, RetainedRecoverySnapshot,
    RetainedRecoverySource,
};
mod runtime_migration;
pub use runtime_migration::*;
mod custom_runtime;
pub use custom_runtime::{
    SandboxCustomRuntimeMigration, SandboxCustomRuntimeOrigin, SandboxCustomRuntimeState,
};
mod retained_runtime_transition;
pub use retained_runtime_transition::{
    next_replacement_runtime_generation, RetainedRuntimePolicyBinding, RetainedRuntimeTransition,
    RetainedRuntimeTransitionRequest,
};
mod runtime_image_origin;
pub use runtime_image_origin::SandboxRuntimeImageOrigin;
mod runtime_update;
mod runtime_update_resume;
pub use runtime_update::{
    runtime_update_agents_are_idle, SandboxRuntimeUpdateRequest, SandboxRuntimeUpdateState,
};
mod runtime_update_service;
pub use runtime_update_service::{SandboxRuntimeUpdateError, SandboxRuntimeUpdateService};
mod runtime_recovery_agents;
mod unpublished_recovery;
mod unpublished_recovery_legacy;
mod unpublished_recovery_reservation;
mod unpublished_recovery_state;
mod unpublished_recovery_transition;
pub use unpublished_recovery::{
    apply_unpublished_runtime_recovery, SandboxRecoveryConversationProof,
    SandboxUnpublishedRecoveryMutation, SandboxUnpublishedRecoveryProof,
    SandboxUnpublishedRecoveryRequest, SandboxUnpublishedRecoverySnapshot,
    SandboxUnpublishedRecoveryVerifier,
};
pub use unpublished_recovery_state::{
    SandboxUnpublishedRecoveryQuiescence, SandboxUnpublishedRecoveryState,
};
mod capacity;
#[allow(clippy::module_inception)]
mod sandbox;
pub use capacity::{OwnerSandboxCapacity, SandboxCapacitySnapshot, SandboxQuotaUsage};
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
mod title_effect;
mod title_sanitize;
mod title_service;
pub use title_effect::{
    AgentTitleEffect, AgentTitleEffectIntent, AgentTitleEffectState, PreparedAgentTitle,
};
mod title_store;
mod warmup;
mod workspace;
mod workspace_prepare_observations;
mod workspace_prepare_recovery;
mod workspace_service;
mod workspace_work;
pub use workspace_prepare_observations::{
    WorkspacePrepareObservation, WorkspacePrepareObservations,
};
pub use workspace_prepare_recovery::{
    WorkspacePrepareRecovery, WorkspacePrepareRecoveryAttempt, WorkspacePrepareRecoveryCompletion,
};
mod worktree;
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
pub use agent_reads::privacy_export;
pub use agent_reads::{
    AgentBufferedEventRequest, AgentConfigurationRead, AgentDetailReading,
    AgentEventStreamAuthorization, AgentEventStreamHeader, AgentEventStreamSnapshot,
    AgentHistoryCursor, AgentHistoryReading, AgentOriginalRequest, AgentOutputObservation,
    AgentReadService, AgentReplayEntry, AgentReplayWindow, AgentResponseBinding,
    AgentResponseObservation, AgentRetainedOutput, AgentRetainedOutputAvailability,
    AgentRetainedOutputPage, AgentSourceReadError, AgentSourceReading, AgentVisibleLogEntry,
    SharedAgentDetail, SharedAgentOrigin, RETAINED_OUTPUT_CHUNK_BYTES,
};
pub use agent_runtime::{
    AgentRuntimeConnection, AgentRuntimeError, AgentRuntimeInput, AgentRuntimeInputError,
    AgentRuntimeInputSender, AgentRuntimePromptResponse, AgentRuntimeRegistry, AgentRuntimeTarget,
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
    AgentConversationError, AgentMessageAssembly, AgentOutputCommit, AgentUpdateOutcome,
    AgentUpdateRecorder, MAX_DURABLE_AGENT_MESSAGE_BYTES,
};
pub use conversation_recorder::{
    AgentHeadSubscription, AgentLiveDelivery, AgentLiveError, AgentLiveFragment,
    AgentLiveFragmentKind, AgentLivePublisher, AgentLiveSource, AgentLiveSubscription,
    AgentOutputCoalescePolicy, DurableAgentUpdateRecorder,
};
pub use credential::{
    InitialServiceConnection, RepositoryCredentialRequirement, SandboxCredentialAuthority,
    SandboxCredentialSource, SandboxCredentialState, SandboxRevokeDebt, SandboxServiceGrant,
    SandboxServiceScopeRevision, SandboxSignInState, ServiceConnectionSelection,
};
pub use credential_prerequisite::{
    FixedSandboxCredentialPrerequisite, SandboxCredentialPrerequisite,
    SandboxCredentialPrerequisiteError,
};
pub use error::SandboxModelError;
pub use event::{
    AgentAvailableCommand, AgentConfigOption, AgentLogEntry, AgentLogKind, AgentPlanStep,
    ExternalApprovalResolution, RecordedToolOutput, SandboxEvent, SandboxEventKind,
};
pub use execution::ProvisioningExecution;
pub use first_message::{
    FirstMessageDelivery, FirstMessageFailure, FirstMessagePreparation, FirstMessagePurpose,
    FirstMessageRecord, MAX_FIRST_MESSAGE_BYTES,
};
pub use first_message_service::{AgentWorkAuthority, FirstMessageDispatchOutcome};
pub use hierarchy::{
    AgentHierarchySummary, HierarchyCounts, HierarchyReadError, HierarchyReadService,
    HierarchyReadStore, OwnerSandboxQuery, SandboxAgentLineage, SandboxDetailPhase,
    SandboxDetailRecord, SandboxDetailTimer, SandboxHierarchyReading, SandboxHierarchyRecord,
    SandboxHierarchySummary, SandboxHierarchyView, SandboxInventoryReading, SandboxInventoryRecord,
    SandboxSnapshotDownload, SharedHierarchyAgent, SharedSandboxHierarchy, SharedSandboxSummary,
    SharedWorkspaceHierarchy, SharedWorkspaceSummary, SharedWorktreeSummary,
    WorkspaceHierarchyReading, WorkspaceHierarchyRecord, WorkspaceHierarchySummary,
    WorkspaceHierarchyView, WorktreeHierarchyReading, WorktreeHierarchySummary,
};
pub use home::SandboxHome;
pub use home_recovery::{SandboxHomeRecovery, SandboxRecoveryPause};
pub use inactivity::{
    has_open_desktop_session, inactivity_pause_notice, is_inactivity_pause_candidate,
    is_platform_stop_pause_candidate, is_quiet_pause_candidate, register_open_desktop_session,
    unregister_open_desktop_session, InactivityPauseRefusal, SandboxAgentActivity,
    SandboxAgentLogActivity, PLATFORM_SPEND_PAUSE_NOTICE,
};
pub use interruption::{SandboxInterruption, SandboxInterruptionCause};
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
    AGENT_TITLE_MAX_ATTEMPTS, AGENT_TITLE_MAX_SCALARS, AGENT_TITLE_RETRY_SECONDS,
    AGENT_TITLE_SYSTEM_PROMPT, EMPTY_AGENT_TITLE_FALLBACK,
};
pub use title_service::{AgentTitleOutcome, AgentTitleService, AgentTitleServiceError};
pub use title_store::{
    AgentTitleClaim, AgentTitleClaimMode, AgentTitleClaimRequest, AgentTitleStore,
    AgentTitleStoreError, InMemoryAgentTitleStore, SetAgentTitleOutcome, StoredAgentTitle,
};
pub use warmup::{
    WarmAgent, WarmSandbox, WarmSandboxPlan, WarmSandboxState, WarmWorkspace, WarmWorkspacePlan,
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
mod always_on_tests;
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
mod runtime_capacity;
#[cfg(test)]
mod runtime_generation_tests;
#[cfg(test)]
mod runtime_update_lifecycle_tests;
#[cfg(test)]
mod runtime_update_resume_tests;
#[cfg(test)]
mod runtime_update_test_support;
#[cfg(test)]
mod runtime_update_tests;
#[cfg(test)]
#[path = "../sandbox_usage_tests.rs"]
mod sandbox_usage_tests;
#[cfg(test)]
mod service_tests;
mod suspended_runtime;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod title_effect_tests;
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
pub use runtime_capacity::RuntimeCapacityError;
pub mod model_credit;
