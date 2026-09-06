//! Layer 3 sandbox lifecycle and ACP conversation boundary.
//!
//! Layer 2 maps its durable model into the neutral request types exported here.
//! Hosted implementations transport those requests to a runtime supervisor;
//! agent-specific filesystem and launch conventions remain in their own module.

mod adapter;
mod agent;
mod capabilities;
mod checkpoint;
mod conversation;
mod conversation_channel;
mod error;
mod runtime;
mod workspace;

pub mod codex;
pub mod hosted;
pub mod provider;
pub mod wire;

pub use adapter::SandboxAdapter;
pub use agent::{
    AgentCloseRequest, AgentHomePlan, AgentHomeRemoveRequest, AgentHomeRequest, AgentLaunch,
    AgentOpenRequest, AgentSkill, AgentSlot, ConversationSnapshot, HomeFile, HomeSymlink,
    PermissionLevel, ReasoningEffort, RuntimeOptions, ThinkingEffort,
};
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
    AvailableCommand, AvailableCommandsUpdate, ContentBlock, ContentChunk, ConversationChannel,
    ConversationId, ConversationInput, ConversationInputSender, ConversationSendOutcome,
    ConversationUpdate, MessageId, PermissionDecision, PermissionLocator, PermissionOption,
    PermissionOptionId, PermissionOptionKind, PermissionRequest, PromptResponse,
    RequestPermissionRequest, SessionUpdate, ToolCall, ToolCallId, ToolCallStatus, ToolCallUpdate,
    ToolCallUpdateFields, ToolKind, TurnEvidence, TurnScopedEvidence, TurnStopReason, TurnUsage,
    UsageUpdate,
};
pub use error::{AdapterError, Result, RuntimeContractMismatch};
pub use runtime::{
    CommandReceipt, CredentialFrame, InstallReceipt, InstallRequest, RuntimeAdoptionRequest,
    RuntimeCredential, RuntimeHandle, RuntimeImage, RuntimeLifecycle, RuntimeObservation,
    RuntimeProvisionRequest, SandboxCapacity, SandboxSpec, DEFAULT_COMMAND_OUTPUT_LIMIT_BYTES,
    DEFAULT_COMMAND_TIMEOUT_SECONDS, DEFAULT_HOME_DIRECTORY, DEFAULT_SUPERVISOR_PROGRAM,
    DEFAULT_TOTAL_TIMEOUT_SECONDS,
};
pub use workspace::{
    SetupReceipt, WorkspaceReceipt, WorkspaceRemoveReceipt, WorkspaceRemoveRequest,
    WorkspaceRequest, WorkspaceSetupRequest, WorkspaceSource, WorktreeReceipt, WorktreeRef,
    WorktreeRemoveReceipt, WorktreeRequest,
};
