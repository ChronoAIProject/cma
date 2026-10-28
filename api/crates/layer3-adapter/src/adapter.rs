//! The kernel-neutral sandbox adapter contract.

use tokio::sync::mpsc;

use crate::{
    AdapterCapabilities, AgentCloseRequest, AgentHomeRemoveRequest, AgentHomeRequest,
    AgentOpenRequest, AgentSlot, CheckpointHoldReceipt, CheckpointHoldRequest, CheckpointReceipt,
    CheckpointRequest, CheckpointWriterResumeReceipt, CheckpointWriterResumeRequest,
    ConversationChannel, ConversationSnapshot, ConversationUpdate, CredentialFrame, InstallReceipt,
    InstallRequest, ResourceReading, RestoreBootstrapReceipt, RestoreBootstrapRequest,
    RestoreRequest, Result, RuntimeAdoptionRequest, RuntimeHandle, RuntimeObservation,
    RuntimeProvisionRequest, SandboxSpec, SetupReceipt, WorkspaceReceipt, WorkspaceRemoveRequest,
    WorkspaceRequest, WorkspaceSetupRequest, WorktreeReceipt, WorktreeRef, WorktreeRequest,
};

/// Drives the complete lifecycle of one sandbox and the agents inside it.
#[async_trait::async_trait]
pub trait SandboxAdapter: Send + Sync {
    /// Optional provider proof and graceful recovery of unpublished runtimes.
    fn runtime_recovery(&self) -> Option<&dyn crate::RuntimeRecovery> {
        None
    }

    /// Abilities proved for the admitted runtime contract.
    fn capabilities(&self) -> AdapterCapabilities;

    /// Optional durable storage authority; absence must never fall back to an empty home.
    fn home_volumes(&self) -> Option<&dyn crate::HomeVolumeStore> {
        None
    }

    /// Persist the returned storage authority before allocating a home. Runtime
    /// selection and storage policy must remain stable across retries.
    async fn reserve_home_request(
        &self,
        _runtime: &RuntimeProvisionRequest,
        home: &crate::HomeVolumeRequest,
    ) -> Result<crate::HomeVolumeRequest> {
        Ok(home.clone())
    }

    /// Verify retained home authority before provider operations that only take a handle.
    fn verify_retained_home_route(
        &self,
        _handle: &RuntimeHandle,
        _home: &crate::HomeVolumeRequest,
    ) -> Result<()> {
        Ok(())
    }

    fn native_pools(&self) -> Option<&dyn crate::agent_sandbox::NativePoolControl> {
        None
    }
    /// Resolve pool authority from the durable home before claim recovery or cleanup.
    fn native_pools_for_home(
        &self,
        _home: &crate::HomeVolumeRequest,
    ) -> Result<Option<&dyn crate::agent_sandbox::NativePoolControl>> {
        Ok(self.native_pools())
    }

    /// Select a provider and an eligible pool before any home allocation. The
    /// caller persists the returned binding (or ordinary home decision).
    async fn select_native_pool(
        &self,
        _request: &RuntimeProvisionRequest,
        _home: &crate::HomeVolumeRequest,
    ) -> Result<Option<crate::agent_sandbox::NativePoolBinding>> {
        Ok(None)
    }

    async fn provision(&self, spec: &SandboxSpec) -> Result<RuntimeHandle>;

    /// Stop the exact runtime Pod while retaining its provider object and volume.
    async fn suspend_runtime(&self, _request: &RuntimeAdoptionRequest) -> Result<()> {
        Err(crate::AdapterError::Unsupported(
            "runtime suspension is unavailable".into(),
        ))
    }

    async fn verify_suspended_runtime(&self, _request: &RuntimeAdoptionRequest) -> Result<()> {
        Err(crate::AdapterError::Unsupported(
            "runtime suspension observation is unavailable".into(),
        ))
    }

    /// Delete a persisted provider identity and all Pods without requiring a live process.
    async fn teardown_retained_runtime(&self, _handle: &RuntimeHandle) -> Result<()> {
        Err(crate::AdapterError::Unsupported(
            "retained runtime deletion is unavailable".into(),
        ))
    }

    /// Resume the same provider object with a reserved generation and admitted service scope.
    async fn resume_runtime(
        &self,
        _request: &RuntimeAdoptionRequest,
        _next: &RuntimeProvisionRequest,
    ) -> Result<RuntimeHandle> {
        Err(crate::AdapterError::Unsupported(
            "runtime resumption is unavailable".into(),
        ))
    }

    /// Explicitly replace only the image of an idle, suspended native runtime.
    /// The successor pins its image and retains the original provider and home.
    async fn upgrade_runtime(
        &self,
        _request: &RuntimeAdoptionRequest,
        _next: &RuntimeProvisionRequest,
    ) -> Result<RuntimeHandle> {
        Err(crate::AdapterError::Unsupported(
            "native runtime upgrades are unavailable".into(),
        ))
    }

    /// Exactly one creation dispatch; failures cannot be upgraded by name-only recovery.
    async fn provision_runtime_receipted(
        &self,
        _request: &RuntimeProvisionRequest,
        _operation_id: &str,
    ) -> Result<crate::RuntimeProvisionReceipt> {
        Err(crate::AdapterError::Unsupported(
            "original provider incarnation receipts are unavailable".into(),
        ))
    }

    /// Delete only the original incarnation and return actual provider effect receipts.
    async fn finalize_runtime(
        &self,
        _original: &crate::RuntimeProvisionReceipt,
        _operation_id: &str,
    ) -> Result<crate::RuntimeFinalityReceipt> {
        Err(crate::AdapterError::Unsupported(
            "exact provider finality is unavailable".into(),
        ))
    }

    /// Read the current process incarnation only for a verified persistent home.
    async fn runtime_status(&self, _handle: &RuntimeHandle) -> Result<crate::RuntimeStatus> {
        Err(crate::AdapterError::Unsupported(
            "persistent runtime status is not supported".into(),
        ))
    }

    /// Provision an exact non-zero runtime generation.
    async fn provision_runtime(&self, request: &RuntimeProvisionRequest) -> Result<RuntimeHandle> {
        if request.generation == 0 {
            return Err(crate::AdapterError::Rejected(
                "runtime generation must be non-zero".into(),
            ));
        }
        if request.generation == 1 {
            return self.provision(&request.spec).await;
        }
        Err(crate::AdapterError::Unsupported(
            "generation-specific provisioning is not supported".into(),
        ))
    }

    /// Optional exact termination evidence. Queried only after runtime loss is confirmed.
    async fn runtime_termination(
        &self,
        _handle: &RuntimeHandle,
    ) -> Result<Option<crate::RuntimeTermination>> {
        Ok(None)
    }

    /// Observe the complete provider inventory or refuse the whole result.
    async fn runtime_inventory(&self) -> Result<Vec<RuntimeObservation>> {
        Err(crate::AdapterError::Unsupported(
            "runtime inventory is not supported".into(),
        ))
    }

    /// Attach local runtime state only after proving one exact provider row.
    async fn adopt_runtime(
        &self,
        _request: &RuntimeAdoptionRequest,
    ) -> Result<Option<RuntimeObservation>> {
        Err(crate::AdapterError::Unsupported(
            "runtime adoption is not supported".into(),
        ))
    }

    /// Collect adoption and process status within one call. Implementations may
    /// share fresh predicates inside this call; effects still obtain fresh fences.
    async fn inspect_runtime(
        &self,
        request: &RuntimeAdoptionRequest,
    ) -> Result<Option<crate::RuntimeInspection>> {
        let Some(observation) = self.adopt_runtime(request).await? else {
            return Ok(None);
        };
        if observation.handle != request.handle
            || observation.sandbox_id != request.spec.sandbox_id
            || observation.generation != request.generation
            || !observation.lifecycle.is_running()
        {
            return Err(crate::AdapterError::Rejected(
                "provider did not adopt the exact bound runtime".into(),
            ));
        }
        let status = if request.spec.home_volume.is_some() {
            Some(self.runtime_status(&request.handle).await?)
        } else {
            None
        };
        Ok(Some(crate::RuntimeInspection {
            observation,
            status,
        }))
    }

    /// Reclaim one exact provider observation without trusting a reused handle.
    async fn reclaim_runtime(&self, _observation: &RuntimeObservation) -> Result<()> {
        Err(crate::AdapterError::Unsupported(
            "observed runtime reclamation is not supported".into(),
        ))
    }

    /// Publish an exact owner upload within the retained home, never an arbitrary path.
    async fn upload_agent_attachment(
        &self,
        _handle: &RuntimeHandle,
        _request: &crate::AgentAttachmentRequest,
        _content: Vec<u8>,
    ) -> Result<crate::AgentAttachmentReceipt> {
        Err(crate::AdapterError::Unsupported(
            "agent attachments are unavailable on this runtime".into(),
        ))
    }

    /// List one directory in the retained runtime home. Paths are relative to
    /// that home and are validated by the runtime supervisor.
    async fn list_agent_files(
        &self,
        _handle: &RuntimeHandle,
        _request: &crate::AgentFileListRequest,
    ) -> Result<crate::AgentFileListReceipt> {
        Err(crate::AdapterError::Unsupported(
            "runtime filesystem access is unavailable".into(),
        ))
    }

    async fn read_agent_file(
        &self,
        _handle: &RuntimeHandle,
        _request: &crate::AgentFileReadRequest,
    ) -> Result<crate::AgentFileReadReceipt> {
        Err(crate::AdapterError::Unsupported(
            "runtime filesystem access is unavailable".into(),
        ))
    }

    async fn write_agent_file(
        &self,
        _handle: &RuntimeHandle,
        _request: &crate::AgentFileWriteRequest,
    ) -> Result<crate::AgentFileWriteReceipt> {
        Err(crate::AdapterError::Unsupported(
            "runtime filesystem access is unavailable".into(),
        ))
    }

    async fn delete_agent_file(
        &self,
        _handle: &RuntimeHandle,
        _request: &crate::AgentFileDeleteRequest,
    ) -> Result<crate::AgentFileDeleteReceipt> {
        Err(crate::AdapterError::Unsupported(
            "runtime filesystem access is unavailable".into(),
        ))
    }

    /// Run sandbox-wide installation before any credential is delivered.
    async fn run_install(
        &self,
        handle: &RuntimeHandle,
        request: &InstallRequest,
    ) -> Result<InstallReceipt>;

    /// Inherit installation from an exact checkpoint without executing commands.
    async fn bootstrap_restore(
        &self,
        _handle: &RuntimeHandle,
        _request: &RestoreBootstrapRequest,
    ) -> Result<RestoreBootstrapReceipt> {
        Err(crate::AdapterError::Unsupported(
            "restore bootstrap is not supported".into(),
        ))
    }

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

    async fn recover_workspace_prepare(
        &self,
        _handle: &RuntimeHandle,
        _request: &crate::WorkspacePrepareRecoveryRequest,
    ) -> Result<crate::WorkspacePrepareRecoveryReceipt> {
        Err(crate::AdapterError::Unsupported(
            "preserving Workspace preparation recovery is unavailable".into(),
        ))
    }

    async fn remove_workspace(
        &self,
        handle: &RuntimeHandle,
        request: &WorkspaceRemoveRequest,
    ) -> Result<()>;

    /// Probe the actual bound supervisor. Contract/image labels are insufficient.
    async fn workspace_removal_support(
        &self,
        _handle: &RuntimeHandle,
    ) -> Result<crate::WorkspaceRemovalSupport> {
        Err(crate::AdapterError::Unsupported(
            "guarded Workspace removal is unavailable".into(),
        ))
    }

    async fn reconcile_workspace_removal(
        &self,
        _handle: &RuntimeHandle,
        _expected_runtime: &crate::RuntimeStatus,
        _request: &WorkspaceRemoveRequest,
    ) -> Result<crate::WorkspaceRemovalObservation> {
        Err(crate::AdapterError::Unsupported(
            "current Workspace removal observation is unavailable".into(),
        ))
    }

    async fn add_worktree(
        &self,
        handle: &RuntimeHandle,
        authority_fence: u64,
        request: &WorktreeRequest,
    ) -> Result<WorktreeReceipt>;

    async fn remove_worktree(
        &self,
        handle: &RuntimeHandle,
        authority_fence: u64,
        request: &WorktreeRef,
    ) -> Result<()>;

    async fn materialize_agent_home(
        &self,
        handle: &RuntimeHandle,
        authority_fence: u64,
        request: &AgentHomeRequest,
    ) -> Result<()>;

    /// Stage verified public skill bytes before user credential binding.
    async fn cache_skill_archive(
        &self,
        _handle: &RuntimeHandle,
        _archive: &crate::SkillArchive,
    ) -> Result<()> {
        Err(crate::AdapterError::Unsupported(
            "skill archive preparation is unavailable".into(),
        ))
    }

    async fn install_agent_skill(
        &self,
        _handle: &RuntimeHandle,
        _authority_fence: u64,
        _request: &crate::AgentSkillInstallRequest,
    ) -> crate::Result<()> {
        Err(crate::AdapterError::Unsupported(
            "live skill installation is unavailable".into(),
        ))
    }

    async fn remove_agent_home(
        &self,
        handle: &RuntimeHandle,
        authority_fence: u64,
        request: &AgentHomeRemoveRequest,
    ) -> Result<()>;

    async fn open_agent(
        &self,
        handle: &RuntimeHandle,
        authority_fence: u64,
        request: &AgentOpenRequest,
        updates: mpsc::Sender<ConversationUpdate>,
    ) -> Result<ConversationChannel>;

    /// Replay the original open operation while attaching its recorded conversation.
    /// The connection identity does not change the durable launch fingerprint.
    async fn reconnect_agent(
        &self,
        handle: &RuntimeHandle,
        authority_fence: u64,
        request: &AgentOpenRequest,
        conversation_id: &str,
        updates: mpsc::Sender<ConversationUpdate>,
    ) -> Result<ConversationChannel>;

    /// Reattach an already admitted native prompt without submitting it again.
    #[allow(clippy::too_many_arguments)]
    async fn reconnect_agent_with_prompt(
        &self,
        handle: &RuntimeHandle,
        authority_fence: u64,
        request: &AgentOpenRequest,
        conversation_id: &str,
        _recovered_prompt: Option<crate::PromptIdentity>,
        updates: mpsc::Sender<ConversationUpdate>,
    ) -> Result<ConversationChannel> {
        self.reconnect_agent(handle, authority_fence, request, conversation_id, updates)
            .await
    }

    async fn close_agent(
        &self,
        handle: &RuntimeHandle,
        authority_fence: u64,
        request: &AgentCloseRequest,
    ) -> Result<()>;

    async fn capture_agent(
        &self,
        handle: &RuntimeHandle,
        agent: &AgentSlot,
    ) -> Result<ConversationSnapshot>;

    /// Export the agent-sandbox-owned session JSONL for one admitted Agent.
    /// Implementations preserve the runtime's bounded NDJSON payload.
    async fn export_agent_history(
        &self,
        _handle: &RuntimeHandle,
        _agent: &AgentSlot,
    ) -> Result<Vec<u8>> {
        Err(crate::AdapterError::Unsupported(
            "agent history export is not supported".into(),
        ))
    }

    /// Exact disposable paths relative to the sandbox home, in canonical order.
    /// Callers supply every retained agent slot, including terminal agents.
    fn checkpoint_exclusions(&self, _agents: &[AgentSlot]) -> Result<Vec<String>> {
        Ok(Vec::new())
    }

    async fn checkpoint(
        &self,
        handle: &RuntimeHandle,
        request: &CheckpointRequest,
    ) -> Result<CheckpointReceipt>;

    /// Capture a checkpoint while retaining the runtime's writer fence.
    async fn checkpoint_for_teardown(
        &self,
        _handle: &RuntimeHandle,
        _request: &CheckpointHoldRequest,
    ) -> Result<CheckpointHoldReceipt> {
        Err(crate::AdapterError::Unsupported(
            "checkpoint writer hold is not supported".into(),
        ))
    }

    /// Revalidate the exact writer hold for an already-published checkpoint.
    /// The caller must compare the receipt with its durable checkpoint identity.
    /// Transports may omit archive transfer; the default replays the capture.
    async fn verify_checkpoint_hold(
        &self,
        handle: &RuntimeHandle,
        request: &CheckpointHoldRequest,
    ) -> Result<CheckpointHoldReceipt> {
        self.checkpoint_for_teardown(handle, request).await
    }

    /// Release a retained checkpoint writer fence after an aborted teardown.
    async fn resume_checkpoint_writers(
        &self,
        _handle: &RuntimeHandle,
        _request: &CheckpointWriterResumeRequest,
    ) -> Result<CheckpointWriterResumeReceipt> {
        Err(crate::AdapterError::Unsupported(
            "checkpoint writer resume is not supported".into(),
        ))
    }

    async fn restore(&self, handle: &RuntimeHandle, request: &RestoreRequest) -> Result<()>;

    async fn resources(&self, handle: &RuntimeHandle) -> Result<ResourceReading>;

    async fn teardown(&self, handle: &RuntimeHandle) -> Result<()>;
}
