//! Codex-specific configuration, home layout, and bridge launch.

mod config;
mod home;

use std::sync::Arc;

use tokio::sync::mpsc;

pub use config::{render_config, CodexConfig};
pub use home::{
    render_agent_home, render_launch, AGENT_HOME_ROOT, BRIDGE_PROGRAM, SHARED_AUTH_PATH,
};

use crate::hosted::{
    HostedAdapter, HostedAgentRuntime, HostedBackend, HostedDeployment, InMemoryHostedBackend,
};
use crate::{
    AdapterCapabilities, AgentCloseRequest, AgentHomePlan, AgentHomeRemoveRequest,
    AgentHomeRequest, AgentLaunch, AgentOpenRequest, AgentSlot, CheckpointHoldReceipt,
    CheckpointHoldRequest, CheckpointReceipt, CheckpointRequest, CheckpointWriterResumeReceipt,
    CheckpointWriterResumeRequest, ConversationChannel, ConversationSnapshot, ConversationUpdate,
    CredentialFrame, InstallReceipt, InstallRequest, ResourceReading, RestoreBootstrapReceipt,
    RestoreBootstrapRequest, RestoreRequest, RuntimeAdoptionRequest, RuntimeHandle,
    RuntimeObservation, RuntimeProvisionRequest, SandboxAdapter, SandboxSpec, SetupReceipt,
    WorkspaceReceipt, WorkspaceRemoveRequest, WorkspaceRequest, WorkspaceSetupRequest,
    WorktreeReceipt, WorktreeRef, WorktreeRequest,
};

/// Configuration for Codex inside a hosted sandbox.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CodexSandboxSpec {
    pub deployment: HostedDeployment,
    pub config: CodexConfig,
}

struct CodexRuntime {
    config: CodexConfig,
}

impl HostedAgentRuntime for CodexRuntime {
    fn home_plan(&self, request: &AgentHomeRequest) -> crate::Result<AgentHomePlan> {
        render_agent_home(request, &self.config)
    }

    fn launch(&self, request: &AgentOpenRequest) -> crate::Result<AgentLaunch> {
        render_launch(request)
    }
}

/// Complete Codex sandbox adapter.
pub struct CodexAdapter {
    hosted: HostedAdapter,
}

impl CodexAdapter {
    pub fn new(backend: Arc<dyn HostedBackend>, spec: CodexSandboxSpec) -> Self {
        let runtime = Arc::new(CodexRuntime {
            config: spec.config,
        });
        Self {
            hosted: HostedAdapter::new(backend, runtime, spec.deployment),
        }
    }

    pub fn with_backend<B>(backend: B, spec: CodexSandboxSpec) -> Self
    where
        B: HostedBackend + 'static,
    {
        Self::new(Arc::new(backend), spec)
    }

    /// Build the adapter and retain an inspectable hosted test double.
    pub fn in_memory(spec: CodexSandboxSpec) -> (Self, InMemoryHostedBackend) {
        let backend = InMemoryHostedBackend::default();
        (Self::with_backend(backend.clone(), spec), backend)
    }

    pub fn hosted(&self) -> &HostedAdapter {
        &self.hosted
    }
}

#[async_trait::async_trait]
impl SandboxAdapter for CodexAdapter {
    fn capabilities(&self) -> AdapterCapabilities {
        self.hosted.capabilities()
    }

    async fn provision(&self, spec: &SandboxSpec) -> crate::Result<RuntimeHandle> {
        self.hosted.provision(spec).await
    }

    async fn provision_runtime(
        &self,
        request: &RuntimeProvisionRequest,
    ) -> crate::Result<RuntimeHandle> {
        self.hosted.provision_runtime(request).await
    }

    async fn runtime_inventory(&self) -> crate::Result<Vec<RuntimeObservation>> {
        self.hosted.runtime_inventory().await
    }

    async fn adopt_runtime(
        &self,
        request: &RuntimeAdoptionRequest,
    ) -> crate::Result<Option<RuntimeObservation>> {
        self.hosted.adopt_runtime(request).await
    }

    async fn run_install(
        &self,
        handle: &RuntimeHandle,
        request: &InstallRequest,
    ) -> crate::Result<InstallReceipt> {
        self.hosted.run_install(handle, request).await
    }

    async fn bootstrap_restore(
        &self,
        handle: &RuntimeHandle,
        request: &RestoreBootstrapRequest,
    ) -> crate::Result<RestoreBootstrapReceipt> {
        self.hosted.bootstrap_restore(handle, request).await
    }

    async fn deliver_credential(
        &self,
        handle: &RuntimeHandle,
        frame: CredentialFrame,
    ) -> crate::Result<()> {
        self.hosted.deliver_credential(handle, frame).await
    }

    async fn prepare_workspace(
        &self,
        handle: &RuntimeHandle,
        request: &WorkspaceRequest,
    ) -> crate::Result<WorkspaceReceipt> {
        self.hosted.prepare_workspace(handle, request).await
    }

    async fn run_workspace_setup(
        &self,
        handle: &RuntimeHandle,
        request: &WorkspaceSetupRequest,
    ) -> crate::Result<SetupReceipt> {
        self.hosted.run_workspace_setup(handle, request).await
    }

    async fn remove_workspace(
        &self,
        handle: &RuntimeHandle,
        request: &WorkspaceRemoveRequest,
    ) -> crate::Result<()> {
        self.hosted.remove_workspace(handle, request).await
    }

    async fn add_worktree(
        &self,
        handle: &RuntimeHandle,
        authority_fence: u64,
        request: &WorktreeRequest,
    ) -> crate::Result<WorktreeReceipt> {
        self.hosted
            .add_worktree(handle, authority_fence, request)
            .await
    }

    async fn remove_worktree(
        &self,
        handle: &RuntimeHandle,
        authority_fence: u64,
        request: &WorktreeRef,
    ) -> crate::Result<()> {
        self.hosted
            .remove_worktree(handle, authority_fence, request)
            .await
    }

    async fn materialize_agent_home(
        &self,
        handle: &RuntimeHandle,
        authority_fence: u64,
        request: &AgentHomeRequest,
    ) -> crate::Result<()> {
        self.hosted
            .materialize_agent_home(handle, authority_fence, request)
            .await
    }

    async fn remove_agent_home(
        &self,
        handle: &RuntimeHandle,
        authority_fence: u64,
        request: &AgentHomeRemoveRequest,
    ) -> crate::Result<()> {
        self.hosted
            .remove_agent_home(handle, authority_fence, request)
            .await
    }

    async fn open_agent(
        &self,
        handle: &RuntimeHandle,
        authority_fence: u64,
        request: &AgentOpenRequest,
        updates: mpsc::Sender<ConversationUpdate>,
    ) -> crate::Result<ConversationChannel> {
        self.hosted
            .open_agent(handle, authority_fence, request, updates)
            .await
    }

    async fn close_agent(
        &self,
        handle: &RuntimeHandle,
        authority_fence: u64,
        request: &AgentCloseRequest,
    ) -> crate::Result<()> {
        self.hosted
            .close_agent(handle, authority_fence, request)
            .await
    }

    async fn capture_agent(
        &self,
        handle: &RuntimeHandle,
        agent: &AgentSlot,
    ) -> crate::Result<ConversationSnapshot> {
        self.hosted.capture_agent(handle, agent).await
    }

    async fn checkpoint(
        &self,
        handle: &RuntimeHandle,
        request: &CheckpointRequest,
    ) -> crate::Result<CheckpointReceipt> {
        self.hosted.checkpoint(handle, request).await
    }

    async fn checkpoint_for_teardown(
        &self,
        handle: &RuntimeHandle,
        request: &CheckpointHoldRequest,
    ) -> crate::Result<CheckpointHoldReceipt> {
        self.hosted.checkpoint_for_teardown(handle, request).await
    }

    async fn resume_checkpoint_writers(
        &self,
        handle: &RuntimeHandle,
        request: &CheckpointWriterResumeRequest,
    ) -> crate::Result<CheckpointWriterResumeReceipt> {
        self.hosted.resume_checkpoint_writers(handle, request).await
    }

    async fn restore(&self, handle: &RuntimeHandle, request: &RestoreRequest) -> crate::Result<()> {
        self.hosted.restore(handle, request).await
    }

    async fn resources(&self, handle: &RuntimeHandle) -> crate::Result<ResourceReading> {
        self.hosted.resources(handle).await
    }

    async fn teardown(&self, handle: &RuntimeHandle) -> crate::Result<()> {
        self.hosted.teardown(handle).await
    }
}

#[cfg(test)]
#[path = "golden_tests.rs"]
mod tests;
