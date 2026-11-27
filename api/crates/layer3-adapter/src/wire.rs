//! Shared contract-3 control frames.
//!
//! Layer 4 consumes these shapes in #807. This crate defines and serializes
//! them, but does not execute filesystem or process behavior.

use serde::{Deserialize, Serialize};

use crate::{
    AgentCloseRequest, AgentHomePlan, AgentHomeRemoveRequest, AgentLaunch, AgentSlot,
    CheckpointHoldReceipt, CheckpointHoldRequest, CheckpointReceipt, CheckpointRequest,
    CheckpointWriterResumeReceipt, CheckpointWriterResumeRequest, ConversationSnapshot,
    CredentialFrame, InstallReceipt, InstallRequest, ResourceReading, RestoreBootstrapReceipt,
    RestoreBootstrapRequest, RestoreRequest, SetupReceipt, WorkspaceReceipt,
    WorkspaceRemoveReceipt, WorkspaceRemoveRequest, WorkspaceRequest, WorkspaceSetupRequest,
    WorktreeReceipt, WorktreeRef, WorktreeRemoveReceipt, WorktreeRequest,
};

/// Opt in to the additive Agent resource capability in RuntimeStatus responses.
/// Old runtimes ignore Accept and retain their original request/response shape.
pub const AGENT_SKILL_UPGRADE_STATUS_MEDIA_TYPE: &str =
    "application/vnd.chrono.agent-skill-upgrade-status.v1+json";

pub const AGENT_RESOURCES_STATUS_MEDIA_TYPE: &str =
    "application/vnd.chrono.cma.agent-resources-status+json";

pub const AGENT_COMPUTER_STATUS_MEDIA_TYPE: &str =
    "application/vnd.chrono.cma.agent-computer-status+json";

pub const AGENT_PROCESS_RESTART_STATUS_MEDIA_TYPE: &str =
    "application/vnd.chrono.cma.agent-process-restart-status.v1+json";
pub const GITHUB_CREDENTIAL_ROUTE_STATUS_MEDIA_TYPE: &str =
    "application/vnd.chrono.cma.github-credential-route-status.v1+json";
pub const GITHUB_REPOSITORY_SCOPE_STATUS_MEDIA_TYPE: &str =
    "application/vnd.chrono.cma.github-repository-scope-status.v1+json";

/// A stable reference to one agent slot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentRef {
    pub agent_id: AgentSlot,
}

/// A runtime mutation paired with the caller's monotonic lifecycle authority.
///
/// The fence is transport authority, not part of the immutable request digest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FencedRequest<T> {
    pub authority_fence: u64,
    pub request: T,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restart_runtime: Option<crate::AgentRestartRuntime>,
}

impl<T> FencedRequest<T> {
    pub fn new(authority_fence: u64, request: T) -> Self {
        Self {
            authority_fence,
            request,
            restart_runtime: None,
        }
    }
}

/// A mutation receipt acknowledging the exact authority fence that executed it.
///
/// Layer 4 stores only the inner receipt, allowing an exact replay at a newer
/// fence to acknowledge that newer authority without changing operation identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FencedReceipt<T> {
    pub authority_fence: u64,
    pub receipt: T,
}

impl<T> FencedReceipt<T> {
    pub fn new(authority_fence: u64, receipt: T) -> Self {
        Self {
            authority_fence,
            receipt,
        }
    }
}

/// Two ordered resource operations retain their original identities and receipts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentResourcesPlan {
    pub worktree: WorktreeRequest,
    pub home: AgentHomePlan,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentResourcesReceipt {
    pub worktree: WorktreeReceipt,
    pub home: AgentHomeReceipt,
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
    pub operation_id: String,
    pub request_sha256: String,
    pub agent_id: AgentSlot,
    pub cwd: String,
    pub home: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
}

/// Correlated proof that an agent home was published.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentHomeReceipt {
    pub operation_id: String,
    pub request_sha256: String,
    pub agent_id: AgentSlot,
    pub path: String,
}

/// Correlated proof that an agent process was stopped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentCloseReceipt {
    pub operation_id: String,
    pub request_sha256: String,
    pub agent_id: AgentSlot,
}

/// Read back the exact Open and Close on one observed supervisor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentProcessCloseProof {
    pub runtime: crate::AgentRestartRuntime,
    pub close: AgentCloseRequest,
    pub open_request_sha256: String,
    pub worktree_path: String,
}

/// Fence a pending restart on its original runtime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentRestartFence {
    pub predecessor: AgentProcessCloseProof,
    pub successor_operation_id: String,
    pub authority_fence: u64,
    /// With a digest, close the exact journaled successor. Without a digest,
    /// advance the predecessor Close fence only if the successor never arrived.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub successor_open_request_sha256: Option<String>,
}

/// Correlated proof that a stopped agent's home was removed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentHomeRemoveReceipt {
    pub operation_id: String,
    pub request_sha256: String,
    pub agent_id: AgentSlot,
}

/// Operation named by a typed runtime failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WireOperation {
    BrowserControl,
    Browser,
    UploadAgentAttachment,
    ListAgentFiles,
    ReadAgentFile,
    WriteAgentFile,
    DeleteAgentFile,
    CacheSkillArchive,
    RuntimeStatus,
    ToolProxyStatus,
    WorkspaceRemovalSupport,
    ReconcileWorkspaceRemoval,
    Install,
    BootstrapRestore,
    PrepareWorkspace,
    RecoverWorkspacePrepare,
    WorkspaceSetup,
    RemoveWorkspace,
    PrepareAgentResources,
    AddWorktree,
    RemoveWorktree,
    MaterializeAgentHome,
    InstallAgentSkill,
    RemoveAgentHome,
    OpenAgent,
    CloseAgent,
    ProveAgentProcessClosed,
    FenceAgentRestart,
    CaptureAgent,
    Checkpoint,
    CheckpointForTeardown,
    ResumeCheckpointWriters,
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

/// Opt-in completion receipts keep long archive validation compatible with
/// contract-3 clients that still require the legacy empty HTTP 204 response.
pub const CHECKPOINT_UPLOAD_RECEIPT_MEDIA_TYPE: &str =
    "application/vnd.cma.checkpoint-receipt+json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum CheckpointUploadResponse {
    Committed {
        object_reference: String,
        sha256: String,
        size_bytes: u64,
    },
    Failure {
        code: String,
        detail: String,
        retryable: bool,
    },
}

/// Requests accepted by a contract-3 runtime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum RequestFrame {
    BrowserControl(crate::BrowserControlRequest),
    Browser(crate::BrowserRequest),
    UploadAgentAttachment(crate::AgentAttachmentUpload),
    ListAgentFiles(crate::AgentFileListRequest),
    ReadAgentFile(crate::AgentFileReadRequest),
    WriteAgentFile(crate::AgentFileWriteRequest),
    DeleteAgentFile(crate::AgentFileDeleteRequest),
    CacheSkillArchive(crate::SkillArchive),
    RuntimeStatus(crate::RuntimeStatusRequest),
    ToolProxyStatus(crate::ToolProxyRequest),
    WorkspaceRemovalSupport(crate::RuntimeStatusRequest),
    ReconcileWorkspaceRemoval(crate::WorkspaceRemovalRequest),
    Install(InstallRequest),
    BootstrapRestore(RestoreBootstrapRequest),
    PrepareWorkspace(WorkspaceRequest),
    RecoverWorkspacePrepare(crate::WorkspacePrepareRecoveryFrame),
    WorkspaceSetup(WorkspaceSetupRequest),
    RemoveWorkspace(WorkspaceRemoveRequest),
    PrepareAgentResources(FencedRequest<AgentResourcesPlan>),
    AddWorktree(FencedRequest<WorktreeRequest>),
    RemoveWorktree(FencedRequest<WorktreeRef>),
    MaterializeAgentHome(FencedRequest<AgentHomePlan>),
    InstallAgentSkill(FencedRequest<crate::AgentSkillInstallRequest>),
    RemoveAgentHome(FencedRequest<AgentHomeRemoveRequest>),
    OpenAgent(FencedRequest<AgentLaunch>),
    CloseAgent(FencedRequest<AgentCloseRequest>),
    ProveAgentProcessClosed(AgentProcessCloseProof),
    FenceAgentRestart(AgentRestartFence),
    CaptureAgent(AgentRef),
    Checkpoint(CheckpointRequest),
    CheckpointForTeardown(CheckpointHoldRequest),
    ResumeCheckpointWriters(CheckpointWriterResumeRequest),
    Restore(RestoreRequest),
    Credential(CredentialFrame),
    CredentialForReceiver {
        receiver: crate::RuntimeStatusRequest,
        boot_id: String,
        credential: CredentialFrame,
    },
    Resources,
}

impl RequestFrame {
    pub fn has_unexpected_restart_runtime(&self) -> bool {
        match self {
            Self::PrepareAgentResources(request) => request.restart_runtime.is_some(),
            Self::AddWorktree(request) => request.restart_runtime.is_some(),
            Self::RemoveWorktree(request) => request.restart_runtime.is_some(),
            Self::MaterializeAgentHome(request) => request.restart_runtime.is_some(),
            Self::InstallAgentSkill(request) => request.restart_runtime.is_some(),
            Self::RemoveAgentHome(request) => request.restart_runtime.is_some(),
            Self::CloseAgent(request) => request.restart_runtime.is_some(),
            Self::OpenAgent(request) => {
                request.restart_runtime.is_some()
                    && crate::AgentProcessAction::Open
                        .incarnation(
                            request.request.agent_id.as_str(),
                            &request.request.operation_id,
                        )
                        .is_none_or(|incarnation| incarnation == 1)
            }
            _ => false,
        }
    }

    pub const fn operation(&self) -> WireOperation {
        match self {
            Self::Browser(_) => WireOperation::Browser,
            Self::BrowserControl(_) => WireOperation::BrowserControl,
            Self::UploadAgentAttachment(_) => WireOperation::UploadAgentAttachment,
            Self::ListAgentFiles(_) => WireOperation::ListAgentFiles,
            Self::ReadAgentFile(_) => WireOperation::ReadAgentFile,
            Self::WriteAgentFile(_) => WireOperation::WriteAgentFile,
            Self::DeleteAgentFile(_) => WireOperation::DeleteAgentFile,
            Self::CacheSkillArchive(_) => WireOperation::CacheSkillArchive,
            Self::RuntimeStatus(_) => WireOperation::RuntimeStatus,
            Self::ToolProxyStatus(_) => WireOperation::ToolProxyStatus,
            Self::WorkspaceRemovalSupport(_) => WireOperation::WorkspaceRemovalSupport,
            Self::ReconcileWorkspaceRemoval(_) => WireOperation::ReconcileWorkspaceRemoval,
            Self::Install(_) => WireOperation::Install,
            Self::BootstrapRestore(_) => WireOperation::BootstrapRestore,
            Self::PrepareWorkspace(_) => WireOperation::PrepareWorkspace,
            Self::RecoverWorkspacePrepare(_) => WireOperation::RecoverWorkspacePrepare,
            Self::WorkspaceSetup(_) => WireOperation::WorkspaceSetup,
            Self::RemoveWorkspace(_) => WireOperation::RemoveWorkspace,
            Self::PrepareAgentResources(_) => WireOperation::PrepareAgentResources,
            Self::AddWorktree(_) => WireOperation::AddWorktree,
            Self::RemoveWorktree(_) => WireOperation::RemoveWorktree,
            Self::MaterializeAgentHome(_) => WireOperation::MaterializeAgentHome,
            Self::InstallAgentSkill(_) => WireOperation::InstallAgentSkill,
            Self::RemoveAgentHome(_) => WireOperation::RemoveAgentHome,
            Self::OpenAgent(_) => WireOperation::OpenAgent,
            Self::CloseAgent(_) => WireOperation::CloseAgent,
            Self::ProveAgentProcessClosed(_) => WireOperation::ProveAgentProcessClosed,
            Self::FenceAgentRestart(_) => WireOperation::FenceAgentRestart,
            Self::CaptureAgent(_) => WireOperation::CaptureAgent,
            Self::Checkpoint(_) => WireOperation::Checkpoint,
            Self::CheckpointForTeardown(_) => WireOperation::CheckpointForTeardown,
            Self::ResumeCheckpointWriters(_) => WireOperation::ResumeCheckpointWriters,
            Self::Restore(_) => WireOperation::Restore,
            Self::Credential(_) | Self::CredentialForReceiver { .. } => WireOperation::Credential,
            Self::Resources => WireOperation::Resources,
        }
    }
}

/// Typed replies returned by a contract-3 runtime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum ResponseFrame {
    BrowserControl(crate::BrowserControlReceipt),
    Browser(crate::BrowserReceipt),
    UploadAgentAttachment(crate::AgentAttachmentReceipt),
    ListAgentFiles(crate::AgentFileListReceipt),
    ReadAgentFile(crate::AgentFileReadReceipt),
    WriteAgentFile(crate::AgentFileWriteReceipt),
    DeleteAgentFile(crate::AgentFileDeleteReceipt),
    CacheSkillArchive { sha256: String },
    RuntimeStatus(crate::RuntimeStatus),
    ToolProxyStatus(crate::ToolProxyStatus),
    WorkspaceRemovalSupport(crate::WorkspaceRemovalSupport),
    ReconcileWorkspaceRemoval(crate::WorkspaceRemovalObservation),
    Install(InstallReceipt),
    BootstrapRestore(RestoreBootstrapReceipt),
    PrepareWorkspace(WorkspaceReceipt),
    RecoverWorkspacePrepare(crate::WorkspacePrepareRecoveryReceipt),
    WorkspaceSetup(SetupReceipt),
    RemoveWorkspace(WorkspaceRemoveReceipt),
    PrepareAgentResources(FencedReceipt<AgentResourcesReceipt>),
    AddWorktree(FencedReceipt<WorktreeReceipt>),
    RemoveWorktree(FencedReceipt<WorktreeRemoveReceipt>),
    MaterializeAgentHome(FencedReceipt<AgentHomeReceipt>),
    InstallAgentSkill(FencedReceipt<crate::AgentSkillInstallReceipt>),
    RemoveAgentHome(FencedReceipt<AgentHomeRemoveReceipt>),
    OpenAgent(FencedReceipt<AgentOpenReceipt>),
    CloseAgent(FencedReceipt<AgentCloseReceipt>),
    ProveAgentProcessClosed(AgentProcessCloseProof),
    FenceAgentRestart(AgentRestartFence),
    CaptureAgent(ConversationSnapshot),
    Checkpoint(CheckpointReceipt),
    CheckpointForTeardown(CheckpointHoldReceipt),
    ResumeCheckpointWriters(CheckpointWriterResumeReceipt),
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
