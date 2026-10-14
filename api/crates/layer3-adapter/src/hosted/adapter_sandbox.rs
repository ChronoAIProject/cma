use super::*;

#[async_trait::async_trait]
impl SandboxAdapter for HostedAdapter {
    async fn reserve_home_request(
        &self,
        runtime: &RuntimeProvisionRequest,
        home: &crate::HomeVolumeRequest,
    ) -> crate::Result<crate::HomeVolumeRequest> {
        let rendered = self
            .deployment
            .render_runtime(&runtime.spec, runtime.generation)?;
        self.backend.reserve_home_request(&rendered, home).await
    }
    fn verify_retained_home_route(
        &self,
        handle: &RuntimeHandle,
        home: &crate::HomeVolumeRequest,
    ) -> crate::Result<()> {
        self.backend.verify_retained_home_route(handle, home)
    }
    fn native_pools(&self) -> Option<&dyn crate::agent_sandbox::NativePoolControl> {
        self.backend.native_pools()
    }
    fn native_pools_for_home(
        &self,
        home: &crate::HomeVolumeRequest,
    ) -> crate::Result<Option<&dyn crate::agent_sandbox::NativePoolControl>> {
        self.backend.native_pools_for_home(home)
    }
    async fn select_native_pool(
        &self,
        request: &RuntimeProvisionRequest,
        home: &crate::HomeVolumeRequest,
    ) -> crate::Result<Option<crate::agent_sandbox::NativePoolBinding>> {
        let rendered = self
            .deployment
            .render_runtime(&request.spec, request.generation)?;
        self.backend
            .select_native_pool(&rendered, &self.deployment.image, home)
            .await
    }
    fn capabilities(&self) -> AdapterCapabilities {
        self.deployment.capabilities()
    }

    fn home_volumes(&self) -> Option<&dyn crate::HomeVolumeStore> {
        self.home_volumes.as_deref()
    }

    async fn provision(&self, spec: &SandboxSpec) -> crate::Result<RuntimeHandle> {
        self.provision_exact(spec, 1).await
    }

    async fn provision_runtime_receipted(
        &self,
        request: &RuntimeProvisionRequest,
        operation_id: &str,
    ) -> crate::Result<crate::RuntimeProvisionReceipt> {
        self.provision_receipted_exact(request, operation_id).await
    }

    async fn finalize_runtime(
        &self,
        original: &crate::RuntimeProvisionReceipt,
        operation_id: &str,
    ) -> crate::Result<crate::RuntimeFinalityReceipt> {
        let receipt = self
            .backend
            .finalize_runtime(original, operation_id)
            .await?;
        receipt.validate()?;
        if receipt.original() != original || receipt.delete_operation_id() != operation_id {
            return Err(AdapterError::Rejected(
                "provider returned another runtime finality receipt".into(),
            ));
        }
        self.states.lock().await.remove(original.handle());
        Ok(receipt)
    }

    async fn teardown_retained_runtime(&self, handle: &RuntimeHandle) -> crate::Result<()> {
        self.backend.teardown_retained(handle).await?;
        self.states.lock().await.remove(handle);
        Ok(())
    }
    async fn suspend_runtime(&self, request: &RuntimeAdoptionRequest) -> crate::Result<()> {
        self.suspend_exact(request, false).await
    }
    async fn verify_suspended_runtime(
        &self,
        request: &RuntimeAdoptionRequest,
    ) -> crate::Result<()> {
        self.suspend_exact(request, true).await
    }
    async fn resume_runtime(
        &self,
        request: &RuntimeAdoptionRequest,
        next: &RuntimeProvisionRequest,
    ) -> crate::Result<RuntimeHandle> {
        self.resume_exact(request, next).await
    }

    async fn inspect_runtime(
        &self,
        request: &RuntimeAdoptionRequest,
    ) -> crate::Result<Option<crate::RuntimeInspection>> {
        self.inspect_exact(request).await
    }

    async fn runtime_status(&self, handle: &RuntimeHandle) -> crate::Result<crate::RuntimeStatus> {
        self.inspect_runtime_status(handle).await
    }

    async fn provision_runtime(
        &self,
        request: &RuntimeProvisionRequest,
    ) -> crate::Result<RuntimeHandle> {
        self.provision_exact(&request.spec, request.generation)
            .await
    }

    async fn runtime_inventory(&self) -> crate::Result<Vec<RuntimeObservation>> {
        self.inventory_exact().await
    }

    async fn adopt_runtime(
        &self,
        request: &RuntimeAdoptionRequest,
    ) -> crate::Result<Option<RuntimeObservation>> {
        self.adopt_exact(request).await
    }

    async fn reclaim_runtime(&self, observation: &RuntimeObservation) -> crate::Result<()> {
        self.reclaim_exact(observation).await
    }

    async fn run_install(
        &self,
        handle: &RuntimeHandle,
        request: &InstallRequest,
    ) -> crate::Result<InstallReceipt> {
        self.run_install_request(handle, request).await
    }

    async fn bootstrap_restore(
        &self,
        handle: &RuntimeHandle,
        request: &RestoreBootstrapRequest,
    ) -> crate::Result<RestoreBootstrapReceipt> {
        let state = self.known_state(handle).await?;
        if request.sandbox_id != state.provision.sandbox_id
            || request.operation_id.trim().is_empty()
            || request.operation_id.len() > 256
            || request.operation_id.chars().any(char::is_control)
            || !request.has_valid_sha256()
            || !request.checkpoint.has_valid_reference()
        {
            return Err(AdapterError::Rejected(
                "restore bootstrap identity is invalid".into(),
            ));
        }
        match self
            .exchange(handle, RequestFrame::BootstrapRestore(request.clone()))
            .await?
        {
            ResponseFrame::BootstrapRestore(receipt)
                if receipt.operation_id == request.operation_id
                    && receipt.request_sha256 == request.request_sha256
                    && receipt.restore_request_sha256 == request.checkpoint.computed_sha256()
                    && receipt.installation_inherited =>
            {
                Ok(receipt)
            }
            ResponseFrame::BootstrapRestore(_) => Err(AdapterError::Rejected(
                "runtime did not acknowledge the exact restore bootstrap".into(),
            )),
            _ => unreachable!("exchange verifies response operation"),
        }
    }

    async fn deliver_credential(
        &self,
        handle: &RuntimeHandle,
        frame: CredentialFrame,
    ) -> crate::Result<()> {
        if frame.operation_id.trim().is_empty()
            || frame.value.expose().is_empty()
            || frame.custody_generation == Some(0)
        {
            return Err(AdapterError::Rejected(
                "credential operation id, value, and non-zero generation are required".into(),
            ));
        }
        self.verify_bound_home(handle).await?;
        let operation_id = frame.operation_id.clone();
        match self.exchange(handle, RequestFrame::Credential(frame)).await {
            Ok(ResponseFrame::Credential(CredentialReceipt {
                operation_id: returned,
                installed: true,
            })) if returned == operation_id => Ok(()),
            Ok(ResponseFrame::Credential(_)) => Err(AdapterError::Rejected(
                "runtime did not acknowledge the exact credential operation".into(),
            )),
            Ok(_) => unreachable!("exchange verifies response operation"),
            Err(error) => Err(error),
        }
    }

    async fn prepare_workspace(
        &self,
        handle: &RuntimeHandle,
        request: &WorkspaceRequest,
    ) -> crate::Result<WorkspaceReceipt> {
        workspace::prepare(self, handle, request).await
    }

    async fn cache_skill_archive(
        &self,
        handle: &RuntimeHandle,
        archive: &crate::SkillArchive,
    ) -> crate::Result<()> {
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

    async fn run_workspace_setup(
        &self,
        handle: &RuntimeHandle,
        request: &WorkspaceSetupRequest,
    ) -> crate::Result<SetupReceipt> {
        workspace::setup(self, handle, request).await
    }

    async fn remove_workspace(
        &self,
        handle: &RuntimeHandle,
        request: &WorkspaceRemoveRequest,
    ) -> crate::Result<()> {
        workspace::remove(self, handle, request).await
    }

    async fn workspace_removal_support(
        &self,
        handle: &RuntimeHandle,
    ) -> crate::Result<crate::WorkspaceRemovalSupport> {
        workspace::removal_support(self, handle).await
    }

    async fn reconcile_workspace_removal(
        &self,
        handle: &RuntimeHandle,
        expected_runtime: &crate::RuntimeStatus,
        request: &WorkspaceRemoveRequest,
    ) -> crate::Result<crate::WorkspaceRemovalObservation> {
        workspace::reconcile_removal(self, handle, expected_runtime, request).await
    }

    async fn add_worktree(
        &self,
        handle: &RuntimeHandle,
        authority_fence: u64,
        request: &WorktreeRequest,
    ) -> crate::Result<WorktreeReceipt> {
        agent::add_worktree(self, handle, authority_fence, request).await
    }

    async fn remove_worktree(
        &self,
        handle: &RuntimeHandle,
        authority_fence: u64,
        request: &WorktreeRef,
    ) -> crate::Result<()> {
        agent::remove_worktree(self, handle, authority_fence, request).await
    }

    async fn materialize_agent_home(
        &self,
        handle: &RuntimeHandle,
        authority_fence: u64,
        request: &AgentHomeRequest,
    ) -> crate::Result<()> {
        agent::materialize_home(self, handle, authority_fence, request).await
    }

    async fn install_agent_skill(
        &self,
        handle: &RuntimeHandle,
        authority_fence: u64,
        request: &crate::AgentSkillInstallRequest,
    ) -> crate::Result<()> {
        agent::install_skill(self, handle, authority_fence, request).await
    }

    async fn remove_agent_home(
        &self,
        handle: &RuntimeHandle,
        authority_fence: u64,
        request: &AgentHomeRemoveRequest,
    ) -> crate::Result<()> {
        agent::remove_home(self, handle, authority_fence, request).await
    }

    async fn open_agent(
        &self,
        handle: &RuntimeHandle,
        authority_fence: u64,
        request: &AgentOpenRequest,
        updates: mpsc::Sender<ConversationUpdate>,
    ) -> crate::Result<ConversationChannel> {
        agent::open_agent(self, handle, authority_fence, request, None, updates).await
    }

    async fn reconnect_agent(
        &self,
        handle: &RuntimeHandle,
        authority_fence: u64,
        request: &AgentOpenRequest,
        conversation_id: &str,
        updates: mpsc::Sender<ConversationUpdate>,
    ) -> crate::Result<ConversationChannel> {
        let recovery = (conversation_id, None);
        agent::reconnect_agent(self, handle, authority_fence, request, recovery, updates).await
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
        agent::reconnect_agent(
            self,
            handle,
            authority_fence,
            request,
            (conversation_id, recovered_prompt),
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
        agent::close_agent(self, handle, authority_fence, request).await
    }

    async fn capture_agent(
        &self,
        handle: &RuntimeHandle,
        agent: &AgentSlot,
    ) -> crate::Result<ConversationSnapshot> {
        self.known_state(handle).await?;
        match self
            .exchange(
                handle,
                RequestFrame::CaptureAgent(AgentRef {
                    agent_id: agent.clone(),
                }),
            )
            .await?
        {
            ResponseFrame::CaptureAgent(snapshot)
                if !snapshot.conversation_id.is_empty()
                    && valid_sha256(&snapshot.archive_sha256)
                    && format!("{:x}", Sha256::digest(&snapshot.archive))
                        == snapshot.archive_sha256 =>
            {
                Ok(snapshot)
            }
            ResponseFrame::CaptureAgent(_) => Err(AdapterError::Rejected(
                "runtime returned an invalid conversation snapshot".into(),
            )),
            _ => unreachable!("exchange verifies response operation"),
        }
    }

    async fn export_agent_history(
        &self,
        handle: &RuntimeHandle,
        agent: &AgentSlot,
    ) -> crate::Result<Vec<u8>> {
        self.backend.export_agent_history(handle, agent).await
    }

    async fn checkpoint(
        &self,
        handle: &RuntimeHandle,
        request: &CheckpointRequest,
    ) -> crate::Result<CheckpointReceipt> {
        self.known_state(handle).await?;
        match self
            .exchange(handle, RequestFrame::Checkpoint(request.clone()))
            .await?
        {
            ResponseFrame::Checkpoint(receipt)
                if receipt.epoch == request.epoch
                    && valid_checkpoint_reference(&receipt.object_reference, &receipt.sha256)
                    && receipt.writers == WriterState::Resumed =>
            {
                Ok(receipt)
            }
            ResponseFrame::Checkpoint(_) => Err(AdapterError::Rejected(
                "runtime returned an invalid checkpoint receipt".into(),
            )),
            _ => unreachable!("exchange verifies response operation"),
        }
    }

    async fn checkpoint_for_teardown(
        &self,
        handle: &RuntimeHandle,
        request: &CheckpointHoldRequest,
    ) -> crate::Result<CheckpointHoldReceipt> {
        self.checkpoint_hold(handle, request, checkpoint::HoldPurpose::Capture)
            .await
    }

    async fn verify_checkpoint_hold(
        &self,
        handle: &RuntimeHandle,
        request: &CheckpointHoldRequest,
    ) -> crate::Result<CheckpointHoldReceipt> {
        self.checkpoint_hold(handle, request, checkpoint::HoldPurpose::VerifyPublished)
            .await
    }

    async fn resume_checkpoint_writers(
        &self,
        handle: &RuntimeHandle,
        request: &CheckpointWriterResumeRequest,
    ) -> crate::Result<CheckpointWriterResumeReceipt> {
        self.known_state(handle).await?;
        if !request.has_valid_writer_resume_token() {
            return Err(AdapterError::Rejected(
                "checkpoint writer resume token must be 64 lowercase hexadecimal characters".into(),
            ));
        }
        self.resume_checkpoint_writer_hold(handle, request).await
    }

    async fn restore(&self, handle: &RuntimeHandle, request: &RestoreRequest) -> crate::Result<()> {
        self.known_state(handle).await?;
        if !valid_checkpoint_reference(&request.object_reference, &request.sha256) {
            return Err(AdapterError::Rejected(
                "restore object reference or digest is invalid".into(),
            ));
        }
        expect_ack(
            self.exchange(handle, RequestFrame::Restore(request.clone()))
                .await?,
        )
    }

    async fn resources(&self, handle: &RuntimeHandle) -> crate::Result<ResourceReading> {
        self.known_state(handle).await?;
        match self.exchange(handle, RequestFrame::Resources).await? {
            ResponseFrame::Resources(reading) => Ok(reading),
            _ => unreachable!("exchange verifies response operation"),
        }
    }

    async fn teardown(&self, handle: &RuntimeHandle) -> crate::Result<()> {
        self.known_state(handle).await?;
        self.backend.teardown(handle).await?;
        self.states.lock().await.remove(handle);
        Ok(())
    }
}
