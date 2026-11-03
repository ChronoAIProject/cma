//! Layer 3 sandbox lifecycle and ACP conversation boundary.
//!
//! Layer 2 maps its durable model into the neutral request types exported here.
//! Hosted implementations transport those requests to a runtime supervisor;
//! agent-specific filesystem and launch conventions remain in their own module.

mod adapter;
mod agent;
mod agent_attachment;
mod agent_filesystem;
mod browser;
pub use browser::{
    BrowserAdapter, BrowserFrame, BrowserObservation, BrowserOperation, BrowserReceipt,
    BrowserRequest, BrowserState,
};
mod agent_skill_install;
pub use agent_skill_install::{AgentSkillInstallReceipt, AgentSkillInstallRequest};
mod agent_provider;
mod capabilities;
mod checkpoint;
mod conversation;
mod conversation_observation;
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
mod prompt_identity;
mod provider_finality;
mod runtime;
pub mod runtime_bootstrap;
mod runtime_output;
mod runtime_status;
mod tool_proxy;
mod workspace;
mod workspace_removal;

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
    reported_tool_text, AvailableCommand, AvailableCommandsUpdate, ContentBlock, ContentChunk,
    ConversationChannel, ConversationId, ConversationInput, ConversationInputGuard,
    ConversationInputSender, ConversationSendOutcome, ConversationSettingsSupport,
    ConversationUpdate, MessageId, PermissionDecision, PermissionLocator, PermissionOption,
    PermissionOptionId, PermissionOptionKind, PermissionRequest, PromptResponse,
    RequestPermissionRequest, SessionUpdate, ToolCall, ToolCallContent, ToolCallId, ToolCallStatus,
    ToolCallUpdate, ToolCallUpdateFields, ToolKind, TurnEvidence, TurnScopedEvidence,
    TurnStopReason, TurnUsage, UsageUpdate,
};
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
pub use runtime::{
    CommandReceipt, CredentialFrame, InstallReceipt, InstallRequest, RuntimeAdoptionRequest,
    RuntimeCredential, RuntimeHandle, RuntimeImage, RuntimeInspection, RuntimeLifecycle,
    RuntimeObservation, RuntimeProvisionRequest, RuntimeTermination, SandboxCapacity, SandboxSpec,
    DEFAULT_COMMAND_OUTPUT_LIMIT_BYTES, DEFAULT_COMMAND_TIMEOUT_SECONDS, DEFAULT_HOME_DIRECTORY,
    DEFAULT_SUPERVISOR_PROGRAM, DEFAULT_TOTAL_TIMEOUT_SECONDS,
};
pub use runtime_output::{RuntimeOutputReceipt, RuntimeOutputSource};
pub use runtime_status::{valid_home_identity, RuntimeStatus, RuntimeStatusRequest};
pub use tool_proxy::{ToolProxyRequest, ToolProxyStatus, LOCAL_TOOL_PROXY_URL};
pub use workspace::{
    SetupReceipt, WorkspaceReceipt, WorkspaceRemoveReceipt, WorkspaceRemoveRequest,
    WorkspaceRequest, WorkspaceSetupRequest, WorkspaceSource, WorktreeReceipt, WorktreeRef,
    WorktreeRemoveReceipt, WorktreeRequest,
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
