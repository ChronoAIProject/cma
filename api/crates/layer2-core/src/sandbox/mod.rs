//! The Sandbox -> Workspace -> Worktree -> Agent domain hierarchy.

mod admin;
mod agent;
mod agent_service;
mod agent_work;
mod conversation;
mod conversation_fold;
mod conversation_fold_support;
mod conversation_recorder;
mod credential;
mod error;
mod event;
mod first_message;
mod limits;
mod provision;
mod queued_message;
#[allow(clippy::module_inception)]
mod sandbox;
mod service;
mod service_trust;
mod status;
pub mod store;
mod title;
mod title_service;
mod title_store;
mod workspace;
mod workspace_service;
mod workspace_work;
mod worktree;

pub use admin::{
    AgentAdminSummary, OperatorError, SandboxAdminQuery, SandboxAdminStore, SandboxAdminSummary,
    SandboxAdminView, SandboxOperatorService, SandboxRetirement, SandboxRetirementResult,
    StoredSandboxRetirement, WorkspaceAdminSummary,
};
pub use agent::{Agent, AgentOrigin, AgentStopReason, ConversationState};
pub use agent_service::{AgentService, AgentServiceError, CreateAgent};
pub use agent_work::{
    AgentCleanupDebt, AgentWorkLease, AgentWorkReceipt, AgentWorkState, AgentWorkStep,
};
pub use conversation::{
    AgentConversationError, AgentMessageAssembly, AgentUpdateOutcome, AgentUpdateRecorder,
    MAX_DURABLE_AGENT_MESSAGE_BYTES,
};
pub use conversation_recorder::DurableAgentUpdateRecorder;
pub use credential::{
    SandboxCredentialAuthority, SandboxCredentialState, SandboxRevokeDebt, SandboxSignInState,
};
pub use error::SandboxModelError;
pub use event::{
    AgentAvailableCommand, AgentConfigOption, AgentLogEntry, AgentLogKind, AgentPlanStep,
    SandboxEvent, SandboxEventKind,
};
pub use first_message::{
    FirstMessageDelivery, FirstMessageFailure, FirstMessageRecord, MAX_FIRST_MESSAGE_BYTES,
};
pub use limits::{enforce_agent_limit, enforce_workspace_limit, SandboxLimitError};
pub use provision::{SandboxWorkLease, SandboxWorkState, SandboxWorkStep};
pub use queued_message::{
    QueuedAgentMessage, QueuedMessageState, MAX_AGENT_MESSAGE_BYTES, MAX_QUEUED_AGENT_MESSAGES,
};
pub use sandbox::{RuntimeBinding, Sandbox, SandboxCheckpointRef, SandboxLimits, TypedFailure};
pub use service::{
    CreateSandbox, SandboxOrigin, SandboxPolicy, SandboxPolicySource, SandboxService,
    SandboxServiceError,
};
pub use status::{AgentStatus, SandboxStatus, WorkspaceStatus, WorktreeStatus};
pub use store::{InMemorySandboxStore, Page, PageRequest, SandboxStoreError};
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
pub use workspace_service::{CreateWorkspace, WorkspaceService, WorkspaceServiceError};
pub use workspace_work::{WorkspaceWorkLease, WorkspaceWorkState, WorkspaceWorkStep};
pub use worktree::Worktree;

#[cfg(test)]
mod agent_model_tests;
#[cfg(test)]
mod agent_service_tests;
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
