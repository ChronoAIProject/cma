use super::*;
use crate::{desktop_ownership::DesktopOwnedRuntime, stale_turn::StaleTurnProducer, Result};

#[async_trait::async_trait]
impl SandboxAdapter for HostedAdapter {
    fn stale_turn_producer(&self, handle: &RuntimeHandle) -> Option<&dyn StaleTurnProducer> {
        self.backend.stale_turn_producer(handle)
    }
    fn startup_binding_headroom(&self, handles: &[RuntimeHandle]) -> Option<usize> {
        self.backend.startup_binding_headroom(handles)
    }
    fn desktop_ownership(&self, handle: &RuntimeHandle) -> Option<&dyn DesktopOwnedRuntime> {
        self.backend.desktop_ownership(handle)
    }
    fn browser(&self) -> Option<&dyn crate::BrowserAdapter> {
        Some(self)
    }
    async fn open_desktop(&self, handle: &RuntimeHandle) -> Result<DesktopConnectionHandle> {
        self.open_desktop(handle).await
    }

    async fn upload_agent_attachment(
        &self,
        handle: &RuntimeHandle,
        request: &crate::AgentAttachmentRequest,
        content: Vec<u8>,
    ) -> Result<crate::AgentAttachmentReceipt> {
        self.upload_attachment(handle, request, content).await
    }

    async fn list_agent_files(
        &self,
        handle: &RuntimeHandle,
        request: &crate::AgentFileListRequest,
    ) -> Result<crate::AgentFileListReceipt> {
        self.list_files(handle, request).await
    }

    async fn read_agent_file(
        &self,
        handle: &RuntimeHandle,
        request: &crate::AgentFileReadRequest,
    ) -> Result<crate::AgentFileReadReceipt> {
        self.read_file(handle, request).await
    }

    async fn write_agent_file(
        &self,
        handle: &RuntimeHandle,
        request: &crate::AgentFileWriteRequest,
    ) -> Result<crate::AgentFileWriteReceipt> {
        self.write_file(handle, request).await
    }

    async fn delete_agent_file(
        &self,
        handle: &RuntimeHandle,
        request: &crate::AgentFileDeleteRequest,
    ) -> Result<crate::AgentFileDeleteReceipt> {
        self.delete_file(handle, request).await
    }

    async fn reserve_home_request(
        &self,
        runtime: &RuntimeProvisionRequest,
        home: &crate::HomeVolumeRequest,
    ) -> Result<crate::HomeVolumeRequest> {
        let rendered = self.deployment.render_request(runtime)?;
        self.backend.reserve_home_request(&rendered, home).await
    }
    fn verify_retained_home_route(
        &self,
        handle: &RuntimeHandle,
        home: &crate::HomeVolumeRequest,
    ) -> Result<()> {
        self.backend.verify_retained_home_route(handle, home)
    }
    fn native_pools(&self) -> Option<&dyn crate::agent_sandbox::NativePoolControl> {
        self.backend.native_pools()
    }
    fn native_pools_for_home(
        &self,
        home: &crate::HomeVolumeRequest,
    ) -> Result<Option<&dyn crate::agent_sandbox::NativePoolControl>> {
        self.backend.native_pools_for_home(home)
    }
    async fn select_native_pool(
        &self,
        request: &RuntimeProvisionRequest,
        home: &crate::HomeVolumeRequest,
    ) -> Result<Option<crate::agent_sandbox::NativePoolBinding>> {
        self.select_native_pool_request(request, home).await
    }
    fn capabilities(&self) -> AdapterCapabilities {
        self.deployment.capabilities()
    }

    fn home_volumes(&self) -> Option<&dyn crate::HomeVolumeStore> {
        self.home_volumes.as_deref()
    }

    async fn set_runtime_deadline(
        &self,
        request: &RuntimeAdoptionRequest,
        until: u64,
    ) -> Result<()> {
        self.deadline_exact(request, until).await
    }

    async fn provision(&self, spec: &SandboxSpec) -> Result<RuntimeHandle> {
        self.provision_exact(spec, 1, None).await
    }

    async fn provision_runtime_receipted(
        &self,
        request: &RuntimeProvisionRequest,
        operation_id: &str,
    ) -> Result<crate::RuntimeProvisionReceipt> {
        self.provision_receipted_exact(request, operation_id).await
    }

    async fn finalize_runtime(
        &self,
        original: &crate::RuntimeProvisionReceipt,
        operation_id: &str,
    ) -> Result<crate::RuntimeFinalityReceipt> {
        self.finalize_runtime_exact(original, operation_id).await
    }

    async fn teardown_retained_runtime(&self, handle: &RuntimeHandle) -> Result<()> {
        self.teardown_retained_exact(handle).await
    }
    fn runtime_retirement(&self) -> Option<&dyn crate::RuntimeRetirement> {
        Some(self)
    }
    async fn suspend_runtime(&self, request: &RuntimeAdoptionRequest) -> Result<()> {
        self.suspend_exact(request, false).await
    }
    async fn verify_suspended_runtime(&self, request: &RuntimeAdoptionRequest) -> Result<()> {
        self.suspend_exact(request, true).await
    }
    fn retained_startup_policy(&self) -> Option<&dyn crate::RetainedStartupPolicyControl> {
        Some(self)
    }
    async fn resume_runtime(
        &self,
        request: &RuntimeAdoptionRequest,
        next: &RuntimeProvisionRequest,
    ) -> Result<RuntimeHandle> {
        self.resume_exact(request, next).await
    }
    async fn upgrade_runtime(
        &self,
        request: &RuntimeAdoptionRequest,
        next: &RuntimeProvisionRequest,
    ) -> Result<RuntimeHandle> {
        self.upgrade_exact(request, next).await
    }
    async fn inspect_runtime(
        &self,
        request: &RuntimeAdoptionRequest,
    ) -> Result<Option<crate::RuntimeInspection>> {
        self.inspect_exact(request).await
    }
    async fn inspect_runtime_after(
        &self,
        request: &RuntimeAdoptionRequest,
        admission: &crate::InspectionAdmission,
    ) -> Result<Option<crate::RuntimeInspection>> {
        self.inspect_exact_after(request, admission).await
    }

    async fn runtime_status(&self, handle: &RuntimeHandle) -> Result<crate::RuntimeStatus> {
        self.inspect_runtime_status(handle).await
    }

    async fn provision_runtime(&self, request: &RuntimeProvisionRequest) -> Result<RuntimeHandle> {
        self.provision_exact(
            &request.released_spec(),
            request.generation,
            request.funded_until,
        )
        .await
    }

    async fn runtime_termination(
        &self,
        handle: &RuntimeHandle,
    ) -> Result<Option<crate::RuntimeTermination>> {
        self.backend.runtime_termination(handle).await
    }

    async fn runtime_inventory(&self) -> Result<Vec<RuntimeObservation>> {
        self.inventory_exact().await
    }

    async fn adopt_runtime(
        &self,
        request: &RuntimeAdoptionRequest,
    ) -> Result<Option<RuntimeObservation>> {
        self.adopt_exact(request).await
    }

    async fn reclaim_runtime(&self, observation: &RuntimeObservation) -> Result<()> {
        self.reclaim_exact(observation).await
    }

    async fn run_install(
        &self,
        handle: &RuntimeHandle,
        request: &InstallRequest,
    ) -> Result<InstallReceipt> {
        self.run_install_request(handle, request).await
    }

    async fn bootstrap_restore(
        &self,
        handle: &RuntimeHandle,
        request: &RestoreBootstrapRequest,
    ) -> Result<RestoreBootstrapReceipt> {
        self.bootstrap_restore_exact(handle, request).await
    }

    async fn deliver_credential(
        &self,
        handle: &RuntimeHandle,
        frame: CredentialFrame,
    ) -> Result<()> {
        self.deliver_credential_exact(handle, frame).await
    }

    async fn prepare_workspace(
        &self,
        handle: &RuntimeHandle,
        request: &WorkspaceRequest,
    ) -> Result<WorkspaceReceipt> {
        workspace::prepare(self, handle, request).await
    }

    async fn cache_skill_archive(
        &self,
        handle: &RuntimeHandle,
        archive: &crate::SkillArchive,
    ) -> Result<()> {
        archive.validate()?;
        match self
            .exchange(handle, RequestFrame::CacheSkillArchive(archive.clone()))
            .await?
        {
            ResponseFrame::CacheSkillArchive { sha256 } if sha256 == archive.sha256 => Ok(()),
            _ => Err(AdapterError::Rejected(
                "skill cache receipt changed its digest".into(),
            )),
        }
    }

    async fn recover_workspace_prepare(
        &self,
        handle: &RuntimeHandle,
        request: &crate::WorkspacePrepareRecoveryRequest,
    ) -> Result<crate::WorkspacePrepareRecoveryReceipt> {
        workspace::recover_prepare(self, handle, request).await
    }

    async fn run_workspace_setup(
        &self,
        handle: &RuntimeHandle,
        request: &WorkspaceSetupRequest,
    ) -> Result<SetupReceipt> {
        workspace::setup(self, handle, request).await
    }

    async fn remove_workspace(
        &self,
        handle: &RuntimeHandle,
        request: &WorkspaceRemoveRequest,
    ) -> Result<()> {
        workspace::remove(self, handle, request).await
    }

    async fn workspace_removal_support(
        &self,
        handle: &RuntimeHandle,
    ) -> Result<crate::WorkspaceRemovalSupport> {
        workspace::removal_support(self, handle).await
    }

    async fn reconcile_workspace_removal(
        &self,
        handle: &RuntimeHandle,
        expected_runtime: &crate::RuntimeStatus,
        request: &WorkspaceRemoveRequest,
    ) -> Result<crate::WorkspaceRemovalObservation> {
        workspace::reconcile_removal(self, handle, expected_runtime, request).await
    }

    async fn prepare_agent_resources(
        &self,
        handle: &RuntimeHandle,
        authority_fence: u64,
        worktree: &WorktreeRequest,
        home: &AgentHomeRequest,
    ) -> Result<WorktreeReceipt> {
        agent_resources::prepare(self, handle, authority_fence, worktree, home).await
    }

    async fn add_worktree(
        &self,
        handle: &RuntimeHandle,
        authority_fence: u64,
        request: &WorktreeRequest,
    ) -> Result<WorktreeReceipt> {
        agent::add_worktree(self, handle, authority_fence, request).await
    }

    async fn remove_worktree(
        &self,
        handle: &RuntimeHandle,
        authority_fence: u64,
        request: &WorktreeRef,
    ) -> Result<()> {
        agent::remove_worktree(self, handle, authority_fence, request).await
    }

    async fn materialize_agent_home(
        &self,
        handle: &RuntimeHandle,
        authority_fence: u64,
        request: &AgentHomeRequest,
    ) -> Result<()> {
        agent::materialize_home(self, handle, authority_fence, request).await
    }

    async fn install_agent_skill(
        &self,
        handle: &RuntimeHandle,
        authority_fence: u64,
        request: &crate::AgentSkillInstallRequest,
    ) -> Result<()> {
        agent::install_skill(self, handle, authority_fence, request).await
    }

    async fn remove_agent_home(
        &self,
        handle: &RuntimeHandle,
        authority_fence: u64,
        request: &AgentHomeRemoveRequest,
    ) -> Result<()> {
        agent::remove_home(self, handle, authority_fence, request).await
    }

    async fn open_agent(
        &self,
        handle: &RuntimeHandle,
        authority_fence: u64,
        request: &AgentOpenRequest,
        updates: mpsc::Sender<ConversationUpdate>,
    ) -> Result<ConversationChannel> {
        agent::open_agent(self, handle, authority_fence, request, None, updates).await
    }

    async fn reconnect_agent(
        &self,
        handle: &RuntimeHandle,
        authority_fence: u64,
        request: &AgentOpenRequest,
        conversation_id: &str,
        updates: mpsc::Sender<ConversationUpdate>,
    ) -> Result<ConversationChannel> {
        let recovery = (conversation_id, None, None);
        agent::reconnect_agent(self, handle, authority_fence, request, recovery, updates).await
    }

    async fn reconnect_agent_for_capacity_recovery(
        &self,
        request: &crate::agent_capacity_recovery::CapacityRecoveryRequest,
        updates: mpsc::Sender<ConversationUpdate>,
    ) -> crate::agent_capacity_recovery::CapacityRecoveryResult<
        crate::agent_capacity_recovery::CapacityRecoveryConnection,
    > {
        crate::hosted::capacity_recovery::reattach(
            self.backend
                .capacity_recovery_transport(&request.target.runtime),
            request,
            updates,
        )
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
    ) -> Result<ConversationChannel> {
        agent::reconnect_agent(
            self,
            handle,
            authority_fence,
            request,
            (conversation_id, recovered_prompt, None),
            updates,
        )
        .await
    }

    async fn reconnect_agent_with_context(
        &self,
        handle: &RuntimeHandle,
        authority_fence: u64,
        request: &AgentOpenRequest,
        conversation_id: &str,
        operation_sequence: u64,
        updates: mpsc::Sender<ConversationUpdate>,
    ) -> Result<ConversationChannel> {
        agent::reconnect_agent(
            self,
            handle,
            authority_fence,
            request,
            (conversation_id, None, Some(operation_sequence)),
            updates,
        )
        .await
    }

    async fn close_agent(
        &self,
        handle: &RuntimeHandle,
        authority_fence: u64,
        request: &AgentCloseRequest,
    ) -> Result<()> {
        agent::close_agent(self, handle, authority_fence, request).await
    }

    async fn capture_agent(
        &self,
        handle: &RuntimeHandle,
        agent: &AgentSlot,
    ) -> Result<ConversationSnapshot> {
        self.capture_agent_exact(handle, agent).await
    }

    async fn export_agent_history(
        &self,
        handle: &RuntimeHandle,
        agent: &AgentSlot,
    ) -> Result<Vec<u8>> {
        self.backend.export_agent_history(handle, agent).await
    }

    async fn checkpoint(
        &self,
        handle: &RuntimeHandle,
        request: &CheckpointRequest,
    ) -> Result<CheckpointReceipt> {
        self.checkpoint_exact(handle, request).await
    }

    async fn checkpoint_for_teardown(
        &self,
        handle: &RuntimeHandle,
        request: &CheckpointHoldRequest,
    ) -> Result<CheckpointHoldReceipt> {
        self.checkpoint_hold(handle, request, checkpoint::HoldPurpose::Capture)
            .await
    }

    async fn verify_checkpoint_hold(
        &self,
        handle: &RuntimeHandle,
        request: &CheckpointHoldRequest,
    ) -> Result<CheckpointHoldReceipt> {
        self.checkpoint_hold(handle, request, checkpoint::HoldPurpose::VerifyPublished)
            .await
    }

    async fn resume_checkpoint_writers(
        &self,
        handle: &RuntimeHandle,
        request: &CheckpointWriterResumeRequest,
    ) -> Result<CheckpointWriterResumeReceipt> {
        self.resume_checkpoint_writers_exact(handle, request).await
    }

    async fn restore(&self, handle: &RuntimeHandle, request: &RestoreRequest) -> Result<()> {
        self.restore_checkpoint(handle, request).await
    }

    async fn resources(&self, handle: &RuntimeHandle) -> Result<ResourceReading> {
        self.resources_exact(handle).await
    }

    async fn teardown(&self, handle: &RuntimeHandle) -> Result<()> {
        self.teardown_exact(handle).await
    }
}
