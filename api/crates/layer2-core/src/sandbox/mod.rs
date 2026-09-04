//! The Sandbox -> Workspace -> Worktree -> Agent domain hierarchy.

mod admin;
mod agent;
mod credential;
mod error;
mod event;
mod limits;
mod provision;
#[allow(clippy::module_inception)]
mod sandbox;
mod service;
mod status;
pub mod store;
mod workspace;
mod worktree;

pub use admin::{
    AgentAdminSummary, OperatorError, SandboxAdminQuery, SandboxAdminStore, SandboxAdminSummary,
    SandboxAdminView, SandboxOperatorService, SandboxRetirement, SandboxRetirementResult,
    StoredSandboxRetirement, WorkspaceAdminSummary,
};
pub use agent::{Agent, AgentOrigin, AgentStopReason, ConversationState, FirstMessageDelivery};
pub use credential::{
    SandboxCredentialAuthority, SandboxCredentialState, SandboxRevokeDebt, SandboxSignInState,
};
pub use error::SandboxModelError;
pub use event::{AgentLogEntry, AgentLogKind, SandboxEvent, SandboxEventKind};
pub use limits::{enforce_agent_limit, enforce_workspace_limit, SandboxLimitError};
pub use provision::{SandboxWorkLease, SandboxWorkState, SandboxWorkStep};
pub use sandbox::{RuntimeBinding, Sandbox, SandboxCheckpointRef, SandboxLimits, TypedFailure};
pub use service::{
    CreateSandbox, SandboxOrigin, SandboxPolicy, SandboxPolicySource, SandboxService,
    SandboxServiceError,
};
pub use status::{AgentStatus, SandboxStatus, WorkspaceStatus, WorktreeStatus};
pub use store::{InMemorySandboxStore, Page, PageRequest, SandboxStoreError};
pub use workspace::{Workspace, WorkspaceSource};
pub use worktree::Worktree;

#[cfg(test)]
mod service_tests;
#[cfg(test)]
mod tests;
