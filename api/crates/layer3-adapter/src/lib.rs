//! Layer 3 sandbox lifecycle and ACP conversation boundary.
//!
//! Layer 2 maps its durable model into the neutral request types exported here.
//! Hosted implementations transport those requests to a runtime supervisor;
//! agent-specific filesystem and launch conventions remain in their own module.

mod adapter;
mod agent;
mod agent_process_identity;
pub use agent_process_identity::AgentProcessAction;
mod agent_attachment;
pub mod agent_capacity_recovery;
mod agent_filesystem;
#[cfg(test)]
mod allocation_probe;
pub mod archive_source;
pub mod binding_diagnostics;
mod browser;
mod browser_control;
pub mod file_read_diagnostics;
pub mod file_receiver_retry;
pub mod permanent_custody;
pub mod stale_turn;
pub use browser::{
    BrowserAdapter, BrowserFrame, BrowserObservation, BrowserOperation, BrowserReceipt,
    BrowserRequest, BrowserState,
};
pub use browser_control::*;
mod agent_skill_install;
pub use agent_skill_install::{
    AgentSkillInstallReceipt, AgentSkillInstallRequest, AgentSkillUpgrade,
    AgentSkillUpgradeRefusal, MAX_AGENT_SKILL_UPGRADE_PREPARATION_ATTEMPTS,
};
mod agent_provider;
mod capabilities;
mod checkpoint;
mod conversation;
mod conversation_observation;
mod tool_diff;
pub use conversation_observation::{
    ConversationDescriptionInput, ConversationDescriptionSource, ConversationMetadata,
    ConversationUsage,
};
mod conversation_channel;
mod error;
mod home_finality;
mod home_snapshot;
mod home_snapshot_export;
mod home_snapshot_job;
mod home_snapshot_view;
mod home_volume;
mod home_volume_router;
mod inspection_admission;
mod runtime_startup;
pub use inspection_admission::{InspectionAdmission, InspectionRelease};
mod inventory_cursor;
pub use inventory_cursor::InventoryCursor;
mod github_repository_scope;
pub mod ingress_budget;
mod prompt_identity;
mod provider_finality;
pub mod recovery_diagnostics;
mod released_runtime;
pub mod resource_execution_protocol;
mod runtime;
pub mod runtime_bootstrap;
pub mod runtime_control_relay;
pub mod runtime_ledger_collection;
pub mod runtime_ledger_maintenance;
pub mod runtime_ledger_retirement;
mod runtime_ledger_status;
mod runtime_output;
mod runtime_output_update;
mod runtime_recovery;
mod runtime_startup_policy;
mod runtime_status;
pub use github_repository_scope::{GithubRepositoryScope, GithubRepositoryScopeNeed};
pub mod runtime_versions;
mod tool_proxy;
mod workspace;
mod workspace_prepare_recovery;
mod workspace_removal;

/// Frames carried by the authenticated desktop WebSocket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DesktopFrame {
    Binary(Vec<u8>),
    Text(String),
    Ping(Vec<u8>),
    Pong(Vec<u8>),
    Close,
}

#[async_trait::async_trait]
pub trait DesktopConnection: Send {
    async fn recv(&mut self) -> Result<Option<DesktopFrame>>;
    async fn send(&mut self, frame: DesktopFrame) -> Result<()>;

    /// An independent control port keeps lease renewal out of the frame loop.
    fn ownership_control(&self) -> Option<std::sync::Arc<dyn DesktopOwnershipControl>> {
        None
    }
}

pub type DesktopConnectionHandle = Box<dyn DesktopConnection>;

pub mod desktop_ownership;
pub use desktop_ownership::{DesktopOwnership, DesktopOwnershipChallenge, DesktopOwnershipControl};

pub mod agent_sandbox;
pub mod codex;
pub mod hosted;
pub mod provider;
pub mod wire;

pub use adapter::SandboxAdapter;
pub use agent::{
    AgentCloseRequest, AgentHomePlan, AgentHomeRemoveRequest, AgentHomeRequest, AgentLaunch,
    AgentOpenRequest, AgentSkill, AgentSlot, ConversationSnapshot, HomeFile, HomeSymlink,
    PermissionLevel, ReasoningEffort, RuntimeOptions, SkillArchive, ThinkingEffort,
};
pub use agent_provider::{AgentProvider, ModelConnectionServices};
pub use capabilities::{
    AdapterCapabilities, RuntimeContract, RUNTIME_CONTRACT_LABEL, RUNTIME_CONTRACT_VERSION,
};
pub use checkpoint::{
    valid_writer_resume_token, writer_resume_tokens_match, CheckpointHoldReceipt,
    CheckpointHoldRequest, CheckpointReceipt, CheckpointRequest, CheckpointWriterResumeReceipt,
    CheckpointWriterResumeRequest, ResourceReading, RestoreBootstrapReceipt,
    RestoreBootstrapRequest, RestoreRequest, WriterState,
};
pub use conversation::{
    has_reported_initial_output, reported_tool_call_text, reported_tool_text, AvailableCommand,
    AvailableCommandsUpdate, ContentBlock, ContentChunk, ConversationChannel, ConversationId,
    ConversationInput, ConversationInputGuard, ConversationInputSender, ConversationSendOutcome,
    ConversationSettingsSupport, ConversationUpdate, MessageId, PermissionDecision,
    PermissionLocator, PermissionOption, PermissionOptionId, PermissionOptionKind,
    PermissionRequest, PromptResponse, RequestPermissionRequest, SessionUpdate, ToolCall,
    ToolCallContent, ToolCallId, ToolCallStatus, ToolCallUpdate, ToolCallUpdateFields, ToolKind,
    TurnEvidence, TurnScopedEvidence, TurnStopReason, TurnUsage, UsageUpdate,
};
pub use error::RuntimeCapacityKind;
pub use error::{AdapterError, Result, RuntimeContractMismatch};
pub use home_finality::{
    HomeCleanupScope, HomeDeleteReceipt, HomeFinalityReceipt, HomeProvisionReceipt,
};
pub use home_snapshot::{
    HomeSnapshotInventoryEntry, HomeSnapshotObservation, HomeSnapshotReceipt, HomeSnapshotRequest,
    HomeSnapshotStore,
};
pub use home_snapshot_export::{
    HomeSnapshotConversationIdentity, HomeSnapshotExportConversation, HomeSnapshotExportReceipt,
    HomeSnapshotExportRequest, MAX_SNAPSHOT_EXPORT_CONVERSATIONS,
};
pub use home_snapshot_job::{
    HomeSnapshotExportJobInventoryEntry, HomeSnapshotExportJobObservation,
    HomeSnapshotExportJobPhase, HomeSnapshotExportJobReceipt, HomeSnapshotExportJobRequest,
    HomeSnapshotExportJobStore,
};
pub use home_snapshot_view::{
    HomeSnapshotViewInventoryEntry, HomeSnapshotViewObservation, HomeSnapshotViewReceipt,
    HomeSnapshotViewStore,
};
pub use home_volume::{
    HomeVolumeAttachment, HomeVolumeMode, HomeVolumePolicy, HomeVolumeReceipt, HomeVolumeRequest,
    HomeVolumeScope, HomeVolumeStore,
};
pub use home_volume_router::RoutedHomeVolumes;
pub use prompt_identity::PromptIdentity;
pub use provider_finality::{
    provider_request_digest, RuntimeFinalityReceipt, RuntimeProviderIncarnation,
    RuntimeProvisionReceipt,
};
pub use released_runtime::{
    released_image, PinnedReleasedRuntime, ReleasedLaneRevisions, ReleasedRuntime,
    ReleasedRuntimeSource, StaleReleasedRuntime,
};
pub use runtime::PlatformServiceRoute;
pub use runtime::{
    CommandReceipt, CredentialFrame, GithubCredentialRoute, InstallReceipt, InstallRequest,
    RuntimeAdoptionRequest, RuntimeCredential, RuntimeHandle, RuntimeImage, RuntimeInspection,
    RuntimeLifecycle, RuntimeObservation, RuntimeProvisionRequest, RuntimeTermination,
    SandboxCapacity, SandboxSpec, DEFAULT_COMMAND_OUTPUT_LIMIT_BYTES,
    DEFAULT_COMMAND_TIMEOUT_SECONDS, DEFAULT_HOME_DIRECTORY, DEFAULT_SUPERVISOR_PROGRAM,
    DEFAULT_TOTAL_TIMEOUT_SECONDS,
};
pub use runtime_ledger_status::{
    RuntimeLedgerCounters, RuntimeLedgerObservation, RuntimeLedgerStatus,
    RUNTIME_LEDGER_STATUS_MEDIA_TYPE,
};
pub use runtime_output::{RuntimeOutputReceipt, RuntimeOutputSource};
pub use runtime_output_update::RuntimeOutputUpdate;
pub use runtime_recovery::{
    NonexecutingRecoveryVerification, RecoveryConversation, RecoveryConversationHistory,
    RuntimeRecovery, RuntimeRecoveryInspection, RuntimeRecoveryInspectionRequest,
    RuntimeRecoveryQuiescence, RuntimeRecoveryQuiescenceRequest,
};
pub use runtime_startup::{RuntimeStartupFailure, RuntimeStartupFailureClass, RuntimeStartupProbe};
pub use runtime_startup_policy::{
    RetainedStartupPolicyControl, RetainedStartupPolicyPredecessor, RetainedStartupPolicyReadback,
    RetainedStartupPolicyUpgrade, RuntimeStartupPolicyObservation,
    RUNTIME_STARTUP_POLICY_STATUS_MEDIA_TYPE,
};
pub use runtime_status::{
    valid_home_identity, AgentRestartRuntime, RuntimeStatus, RuntimeStatusRequest,
};
pub use tool_proxy::{ToolProxyRequest, ToolProxyStatus, LOCAL_TOOL_PROXY_URL};
pub use workspace::{
    public_repository_archive_url, SetupReceipt, WorkspaceReceipt, WorkspaceRemoveReceipt,
    WorkspaceRemoveRequest, WorkspaceRequest, WorkspaceSetupRequest, WorkspaceSource,
    WorktreeReceipt, WorktreeRef, WorktreeRemoveReceipt, WorktreeRequest,
};
pub use workspace_prepare_recovery::{
    WorkspacePrepareRecoveryFrame, WorkspacePrepareRecoveryReceipt, WorkspacePrepareRecoveryRequest,
};
pub use workspace_removal::{
    same_runtime_writer, WorkspaceRemovalObservation, WorkspaceRemovalOutcome,
    WorkspaceRemovalRequest, WorkspaceRemovalSupport, WORKSPACE_REMOVAL_PROTOCOL,
};

pub use agent_attachment::{
    AgentAttachmentReceipt, AgentAttachmentRequest, AgentAttachmentUpload,
    AGENT_ATTACHMENT_DIRECTORY, MAX_AGENT_ATTACHMENT_BYTES,
};
pub use agent_filesystem::{
    valid_identity as valid_agent_filesystem_identity, valid_path as valid_agent_file_path,
    AgentFileDeleteReceipt, AgentFileDeleteRequest, AgentFileEntry, AgentFileKind,
    AgentFileListReceipt, AgentFileListRequest, AgentFileReadReceipt, AgentFileReadRequest,
    AgentFileWriteReceipt, AgentFileWriteRequest, MAX_AGENT_FILE_BYTES, MAX_AGENT_FILE_ENTRIES,
};
pub mod owner_control;
pub mod owner_protocol;
pub mod receiver_failure;
mod runtime_retirement;
pub use runtime_retirement::{RetirementPod, RuntimeRetirement};
pub mod sandbox_metrics;

// Shared immutable archive locator; core persistence and runtime use one shape.
mod runtime_ledger_collection_artifact;
pub use runtime_ledger_collection_artifact::{
    LedgerCollectionArtifactReferenceV1, LEDGER_COLLECTION_ARTIFACT_FORMAT,
};
