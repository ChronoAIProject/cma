//! Shared contract-2 control frames.
//!
//! Layer 4 consumes these shapes in #807. This crate defines and serializes
//! them, but does not execute filesystem or process behavior.

use serde::{Deserialize, Serialize};

use crate::{
    AgentHomePlan, AgentLaunch, AgentSlot, CheckpointReceipt, CheckpointRequest,
    ConversationSnapshot, CredentialFrame, InstallReceipt, InstallRequest, ResourceReading,
    RestoreRequest, SetupReceipt, WorkspaceReceipt, WorkspaceRequest, WorkspaceSetupRequest,
    WorktreeReceipt, WorktreeRef, WorktreeRequest,
};

/// A stable reference to one workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceRef {
    pub name: String,
}

/// A stable reference to one agent slot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentRef {
    pub agent_id: AgentSlot,
}

/// Generic successful acknowledgement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Acknowledgement {
    pub accepted: bool,
}

impl Default for Acknowledgement {
    fn default() -> Self {
        Self { accepted: true }
    }
}

/// Credential installation acknowledgement correlated with the request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CredentialReceipt {
    pub operation_id: String,
    pub installed: bool,
}

/// Agent process-open acknowledgement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentOpenReceipt {
    pub agent_id: AgentSlot,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
}

/// Operation named by a typed runtime failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WireOperation {
    Install,
    PrepareWorkspace,
    WorkspaceSetup,
    RemoveWorkspace,
    AddWorktree,
    RemoveWorktree,
    MaterializeAgentHome,
    OpenAgent,
    CloseAgent,
    CaptureAgent,
    Checkpoint,
    Restore,
    Credential,
    Resources,
}

/// A bounded, non-secret runtime refusal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WireFailure {
    pub operation: WireOperation,
    pub code: String,
    pub detail: String,
    pub retryable: bool,
}

/// Requests accepted by a contract-2 runtime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum RequestFrame {
    Install(InstallRequest),
    PrepareWorkspace(WorkspaceRequest),
    WorkspaceSetup(WorkspaceSetupRequest),
    RemoveWorkspace(WorkspaceRef),
    AddWorktree(WorktreeRequest),
    RemoveWorktree(WorktreeRef),
    MaterializeAgentHome(AgentHomePlan),
    OpenAgent(AgentLaunch),
    CloseAgent(AgentRef),
    CaptureAgent(AgentRef),
    Checkpoint(CheckpointRequest),
    Restore(RestoreRequest),
    Credential(CredentialFrame),
    Resources,
}

impl RequestFrame {
    pub const fn operation(&self) -> WireOperation {
        match self {
            Self::Install(_) => WireOperation::Install,
            Self::PrepareWorkspace(_) => WireOperation::PrepareWorkspace,
            Self::WorkspaceSetup(_) => WireOperation::WorkspaceSetup,
            Self::RemoveWorkspace(_) => WireOperation::RemoveWorkspace,
            Self::AddWorktree(_) => WireOperation::AddWorktree,
            Self::RemoveWorktree(_) => WireOperation::RemoveWorktree,
            Self::MaterializeAgentHome(_) => WireOperation::MaterializeAgentHome,
            Self::OpenAgent(_) => WireOperation::OpenAgent,
            Self::CloseAgent(_) => WireOperation::CloseAgent,
            Self::CaptureAgent(_) => WireOperation::CaptureAgent,
            Self::Checkpoint(_) => WireOperation::Checkpoint,
            Self::Restore(_) => WireOperation::Restore,
            Self::Credential(_) => WireOperation::Credential,
            Self::Resources => WireOperation::Resources,
        }
    }
}

/// Typed replies returned by a contract-2 runtime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum ResponseFrame {
    Install(InstallReceipt),
    PrepareWorkspace(WorkspaceReceipt),
    WorkspaceSetup(SetupReceipt),
    RemoveWorkspace(Acknowledgement),
    AddWorktree(WorktreeReceipt),
    RemoveWorktree(Acknowledgement),
    MaterializeAgentHome(Acknowledgement),
    OpenAgent(AgentOpenReceipt),
    CloseAgent(Acknowledgement),
    CaptureAgent(ConversationSnapshot),
    Checkpoint(CheckpointReceipt),
    Restore(Acknowledgement),
    Credential(CredentialReceipt),
    Resources(ResourceReading),
    Failure(WireFailure),
}

impl ResponseFrame {
    pub fn failure(&self) -> Option<&WireFailure> {
        match self {
            Self::Failure(failure) => Some(failure),
            _ => None,
        }
    }
}

#[cfg(test)]
#[path = "wire_tests.rs"]
mod tests;
