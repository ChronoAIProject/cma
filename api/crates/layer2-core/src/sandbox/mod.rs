//! The Sandbox -> Workspace -> Worktree -> Agent domain hierarchy.

mod agent;
mod error;
mod event;
#[allow(clippy::module_inception)]
mod sandbox;
mod status;
pub mod store;
mod workspace;
mod worktree;

pub use agent::{Agent, AgentOrigin, AgentStopReason, ConversationState, FirstMessageDelivery};
pub use error::SandboxModelError;
pub use event::{AgentLogEntry, AgentLogKind, SandboxEvent, SandboxEventKind};
pub use sandbox::{
    RuntimeBinding, Sandbox, SandboxCheckpointRef, SandboxCredentialState, SandboxLimits,
    TypedFailure,
};
pub use status::{AgentStatus, SandboxStatus, WorkspaceStatus, WorktreeStatus};
pub use store::{InMemorySandboxStore, Page, PageRequest, SandboxStoreError};
pub use workspace::{Workspace, WorkspaceSource};
pub use worktree::Worktree;

#[cfg(test)]
mod tests;
