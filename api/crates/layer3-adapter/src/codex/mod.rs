//! Codex-specific configuration, home layout, and bridge launch.

mod config;
mod connections;
mod construction;
mod home;
mod hosted_runtime;
use hosted_runtime::CodexRuntime;

use std::sync::Arc;

use tokio::sync::mpsc;

pub use config::{render_config, CodexConfig};
pub use connections::{CODEX_SUBSCRIPTION_CONNECTION, CONNECTION_SERVICES, PUBLIC_CODEX_SERVICE};
pub(crate) use home::RESERVED_ENVIRONMENT_NAMES;
pub use home::{
    checkpoint_exclusions, render_agent_home, render_launch, AGENT_HOME_ROOT, BRIDGE_PROGRAM,
    SHARED_AUTH_PATH,
};

use crate::hosted::{
    HostedAdapter, HostedAgentOpenSettings, HostedAgentRuntime, HostedBackend, HostedDeployment,
    InMemoryHostedBackend,
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

/// Complete Codex sandbox adapter.
pub struct CodexAdapter {
    hosted: HostedAdapter,
}

#[async_trait::async_trait]
impl SandboxAdapter for CodexAdapter {
    async fn upload_agent_attachment(
        &self,
        handle: &RuntimeHandle,
        request: &crate::AgentAttachmentRequest,
        content: Vec<u8>,
    ) -> crate::Result<crate::AgentAttachmentReceipt> {
        self.hosted
            .upload_agent_attachment(handle, request, content)
            .await
    }

    async fn reserve_home_request(
        &self,
        runtime: &RuntimeProvisionRequest,
        home: &crate::HomeVolumeRequest,
    ) -> crate::Result<crate::HomeVolumeRequest> {
        self.hosted.reserve_home_request(runtime, home).await
    }
    fn verify_retained_home_route(
        &self,
        handle: &RuntimeHandle,
        home: &crate::HomeVolumeRequest,
    ) -> crate::Result<()> {
        self.hosted.verify_retained_home_route(handle, home)
    }
    fn native_pools(&self) -> Option<&dyn crate::agent_sandbox::NativePoolControl> {
        self.hosted.native_pools()
    }
    fn native_pools_for_home(
        &self,
        home: &crate::HomeVolumeRequest,
    ) -> crate::Result<Option<&dyn crate::agent_sandbox::NativePoolControl>> {
        self.hosted.native_pools_for_home(home)
    }
    async fn select_native_pool(
        &self,
        request: &RuntimeProvisionRequest,
        home: &crate::HomeVolumeRequest,
    ) -> crate::Result<Option<crate::agent_sandbox::NativePoolBinding>> {
        self.hosted.select_native_pool(request, home).await
    }
    fn capabilities(&self) -> AdapterCapabilities {
        self.hosted.capabilities()
    }

    fn home_volumes(&self) -> Option<&dyn crate::HomeVolumeStore> {
        self.hosted.home_volumes()
    }

    async fn provision(&self, spec: &SandboxSpec) -> crate::Result<RuntimeHandle> {
        self.hosted.provision(spec).await
    }

    async fn teardown_retained_runtime(&self, handle: &RuntimeHandle) -> crate::Result<()> {
        self.hosted.teardown_retained_runtime(handle).await
    }
    async fn suspend_runtime(&self, request: &RuntimeAdoptionRequest) -> crate::Result<()> {
        self.hosted.suspend_runtime(request).await
    }
    async fn verify_suspended_runtime(
        &self,
        request: &RuntimeAdoptionRequest,
    ) -> crate::Result<()> {
        self.hosted.verify_suspended_runtime(request).await
    }
    async fn resume_runtime(
        &self,
        request: &RuntimeAdoptionRequest,
        next: &RuntimeProvisionRequest,
    ) -> crate::Result<RuntimeHandle> {
        self.hosted.resume_runtime(request, next).await
    }
    async fn upgrade_runtime(
        &self,
        request: &RuntimeAdoptionRequest,
        next: &RuntimeProvisionRequest,
    ) -> crate::Result<RuntimeHandle> {
        self.hosted.upgrade_runtime(request, next).await
    }

    async fn runtime_status(&self, handle: &RuntimeHandle) -> crate::Result<crate::RuntimeStatus> {
        self.hosted.runtime_status(handle).await
    }

    async fn provision_runtime(
        &self,
        request: &RuntimeProvisionRequest,
    ) -> crate::Result<RuntimeHandle> {
        self.hosted.provision_runtime(request).await
    }

    async fn provision_runtime_receipted(
        &self,
        request: &RuntimeProvisionRequest,
        operation_id: &str,
    ) -> crate::Result<crate::RuntimeProvisionReceipt> {
        self.hosted
            .provision_runtime_receipted(request, operation_id)
            .await
    }

    async fn finalize_runtime(
        &self,
        original: &crate::RuntimeProvisionReceipt,
        operation_id: &str,
    ) -> crate::Result<crate::RuntimeFinalityReceipt> {
        self.hosted.finalize_runtime(original, operation_id).await
    }

    async fn runtime_termination(
        &self,
        handle: &RuntimeHandle,
    ) -> crate::Result<Option<crate::RuntimeTermination>> {
        self.hosted.runtime_termination(handle).await
    }

    async fn runtime_inventory(&self) -> crate::Result<Vec<RuntimeObservation>> {
        self.hosted.runtime_inventory().await
    }

    async fn inspect_runtime(
        &self,
        request: &RuntimeAdoptionRequest,
    ) -> crate::Result<Option<crate::RuntimeInspection>> {
        self.hosted.inspect_runtime(request).await
    }

    async fn adopt_runtime(
        &self,
        request: &RuntimeAdoptionRequest,
    ) -> crate::Result<Option<RuntimeObservation>> {
        self.hosted.adopt_runtime(request).await
    }

    async fn reclaim_runtime(&self, observation: &RuntimeObservation) -> crate::Result<()> {
        self.hosted.reclaim_runtime(observation).await
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

    async fn workspace_removal_support(
        &self,
        handle: &RuntimeHandle,
    ) -> crate::Result<crate::WorkspaceRemovalSupport> {
        self.hosted.workspace_removal_support(handle).await
    }

    async fn reconcile_workspace_removal(
        &self,
        handle: &RuntimeHandle,
        expected_runtime: &crate::RuntimeStatus,
        request: &WorkspaceRemoveRequest,
    ) -> crate::Result<crate::WorkspaceRemovalObservation> {
        self.hosted
            .reconcile_workspace_removal(handle, expected_runtime, request)
            .await
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

    async fn cache_skill_archive(
        &self,
        handle: &RuntimeHandle,
        archive: &crate::SkillArchive,
    ) -> crate::Result<()> {
        self.hosted.cache_skill_archive(handle, archive).await
    }

    async fn install_agent_skill(
        &self,
        handle: &RuntimeHandle,
        authority_fence: u64,
        request: &crate::AgentSkillInstallRequest,
    ) -> crate::Result<()> {
        self.hosted
            .install_agent_skill(handle, authority_fence, request)
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

    async fn reconnect_agent(
        &self,
        handle: &RuntimeHandle,
        authority_fence: u64,
        request: &AgentOpenRequest,
        conversation_id: &str,
        updates: mpsc::Sender<ConversationUpdate>,
    ) -> crate::Result<ConversationChannel> {
        self.hosted
            .reconnect_agent(handle, authority_fence, request, conversation_id, updates)
            .await
    }

    async fn reconnect_agent_with_prompt(
        &self,
        handle: &RuntimeHandle,
        authority_fence: u64,
        request: &AgentOpenRequest,
        conversation_id: &str,
        recovered_prompt: Option<crate::PromptIdentity>,
        updates: mpsc::Sender<ConversationUpdate>,
    ) -> crate::Result<ConversationChannel> {
        self.hosted
            .reconnect_agent_with_prompt(
                handle,
                authority_fence,
                request,
                conversation_id,
                recovered_prompt,
                updates,
            )
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

    async fn export_agent_history(
        &self,
        handle: &RuntimeHandle,
        agent: &AgentSlot,
    ) -> crate::Result<Vec<u8>> {
        self.hosted.export_agent_history(handle, agent).await
    }

    fn checkpoint_exclusions(&self, agents: &[AgentSlot]) -> crate::Result<Vec<String>> {
        home::checkpoint_exclusions(agents)
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

    async fn verify_checkpoint_hold(
        &self,
        handle: &RuntimeHandle,
        request: &CheckpointHoldRequest,
    ) -> crate::Result<CheckpointHoldReceipt> {
        self.hosted.verify_checkpoint_hold(handle, request).await
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

#[cfg(test)]
mod attachments_tests;

#[cfg(test)]
mod termination_tests;
