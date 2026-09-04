//! The kernel-neutral sandbox adapter contract.

use tokio::sync::mpsc;

use crate::{
    AdapterCapabilities, AgentHomeRequest, AgentOpenRequest, AgentSlot, CheckpointReceipt,
    CheckpointRequest, ConversationChannel, ConversationSnapshot, ConversationUpdate,
    CredentialFrame, InstallReceipt, InstallRequest, ResourceReading, RestoreRequest, Result,
    RuntimeHandle, SandboxSpec, SetupReceipt, WorkspaceReceipt, WorkspaceRequest,
    WorkspaceSetupRequest, WorktreeReceipt, WorktreeRef, WorktreeRequest,
};

/// Drives the complete lifecycle of one sandbox and the agents inside it.
#[async_trait::async_trait]
pub trait SandboxAdapter: Send + Sync {
    /// Abilities proved for the admitted runtime contract.
    fn capabilities(&self) -> AdapterCapabilities;

    async fn provision(&self, spec: &SandboxSpec) -> Result<RuntimeHandle>;

    /// Run sandbox-wide installation before any credential is delivered.
    async fn run_install(
        &self,
        handle: &RuntimeHandle,
        request: &InstallRequest,
    ) -> Result<InstallReceipt>;

    async fn deliver_credential(
        &self,
        handle: &RuntimeHandle,
        frame: CredentialFrame,
    ) -> Result<()>;

    async fn prepare_workspace(
        &self,
        handle: &RuntimeHandle,
        request: &WorkspaceRequest,
    ) -> Result<WorkspaceReceipt>;

    async fn run_workspace_setup(
        &self,
        handle: &RuntimeHandle,
        request: &WorkspaceSetupRequest,
    ) -> Result<SetupReceipt>;

    async fn remove_workspace(&self, handle: &RuntimeHandle, name: &str) -> Result<()>;

    async fn add_worktree(
        &self,
        handle: &RuntimeHandle,
        request: &WorktreeRequest,
    ) -> Result<WorktreeReceipt>;

    async fn remove_worktree(&self, handle: &RuntimeHandle, request: &WorktreeRef) -> Result<()>;

    async fn materialize_agent_home(
        &self,
        handle: &RuntimeHandle,
        request: &AgentHomeRequest,
    ) -> Result<()>;

    async fn open_agent(
        &self,
        handle: &RuntimeHandle,
        request: &AgentOpenRequest,
        updates: mpsc::Sender<ConversationUpdate>,
    ) -> Result<ConversationChannel>;

    async fn close_agent(&self, handle: &RuntimeHandle, agent: &AgentSlot) -> Result<()>;

    async fn capture_agent(
        &self,
        handle: &RuntimeHandle,
        agent: &AgentSlot,
    ) -> Result<ConversationSnapshot>;

    async fn checkpoint(
        &self,
        handle: &RuntimeHandle,
        request: &CheckpointRequest,
    ) -> Result<CheckpointReceipt>;

    async fn restore(&self, handle: &RuntimeHandle, request: &RestoreRequest) -> Result<()>;

    async fn resources(&self, handle: &RuntimeHandle) -> Result<ResourceReading>;

    async fn teardown(&self, handle: &RuntimeHandle) -> Result<()>;
}
