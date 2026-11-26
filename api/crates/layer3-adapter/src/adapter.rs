//! The kernel-neutral sandbox adapter contract.
use crate::desktop_ownership::{challenge, open, DesktopOwnedRuntime};
use crate::stale_turn::StaleTurnProducer;
use tokio::sync::mpsc;
mod runtime_inspection;
mod runtime_provision;
mod validation;
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
use validation::{require_admitted_context, unsupported};
/// Drives the complete lifecycle of one sandbox and the agents inside it.
#[async_trait::async_trait]
pub trait SandboxAdapter: Send + Sync {
    /// Advertise only for a qualified runtime producer seal.
    fn stale_turn_producer(&self, _: &RuntimeHandle) -> Option<&dyn StaleTurnProducer> {
        None
    }
    fn browser(&self) -> Option<&dyn crate::BrowserAdapter> {
        None
    }
    /// Optional provider proof and graceful recovery of unpublished runtimes.
    fn runtime_recovery(&self) -> Option<&dyn crate::RuntimeRecovery> {
        None
    }
    /// Remaining cache entries for an exclusively owned startup batch; not a reservation.
    /// Unknown adapters retain serial initialization. No concurrent cache writers may run.
    fn startup_binding_headroom(&self, _handles: &[RuntimeHandle]) -> Option<usize> {
        None
    }
    fn capabilities(&self) -> AdapterCapabilities;
    async fn open_desktop(&self, _: &RuntimeHandle) -> Result<crate::DesktopConnectionHandle> {
        Err(unsupported("runtime desktop is unavailable"))
    }
    fn desktop_ownership(&self, _: &RuntimeHandle) -> Option<&dyn DesktopOwnedRuntime> {
        None
    }
    async fn desktop_ownership_challenge(
        &self,
        handle: &RuntimeHandle,
    ) -> Result<crate::DesktopOwnershipChallenge> {
        challenge(self.desktop_ownership(handle), handle).await
    }
    async fn open_desktop_owned(
        &self,
        handle: &RuntimeHandle,
        owner: &crate::DesktopOwnership,
        challenge: &crate::DesktopOwnershipChallenge,
    ) -> Result<crate::DesktopConnectionHandle> {
        open(self.desktop_ownership(handle), handle, owner, challenge).await
    }
    /// Optional durable storage authority; absence must never fall back to an empty home.
    fn home_volumes(&self) -> Option<&dyn crate::HomeVolumeStore> {
        None
    }
    /// Persist home authority before allocation; retain runtime and storage policy on retry.
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
    /// Select an eligible provider/pool before allocation; persist the returned binding.
    async fn select_native_pool(
        &self,
        _request: &RuntimeProvisionRequest,
        _home: &crate::HomeVolumeRequest,
    ) -> Result<Option<crate::agent_sandbox::NativePoolBinding>> {
        Ok(None)
    }
    /// Bound an adopted runtime to confirmed prepaid coverage. Unsupported
    /// providers must fail closed; callers cannot replace this with a timer.
    async fn set_runtime_deadline(&self, _: &RuntimeAdoptionRequest, _: u64) -> Result<()> {
        Err(unsupported("funded runtime deadlines are unavailable"))
    }
    async fn provision(&self, spec: &SandboxSpec) -> Result<RuntimeHandle>;
    /// Stop the exact runtime Pod while retaining its provider object and volume.
    async fn suspend_runtime(&self, _request: &RuntimeAdoptionRequest) -> Result<()> {
        Err(unsupported("runtime suspension is unavailable"))
    }
    async fn verify_suspended_runtime(&self, _request: &RuntimeAdoptionRequest) -> Result<()> {
        Err(unsupported("runtime suspension observation is unavailable"))
    }
    /// Optional startup policy control for an admitted retained home.
    fn retained_startup_policy(&self) -> Option<&dyn crate::RetainedStartupPolicyControl> {
        None
    }
    /// Delete a persisted provider identity and all Pods without requiring a live process.
    async fn teardown_retained_runtime(&self, _handle: &RuntimeHandle) -> Result<()> {
        Err(unsupported("retained runtime deletion is unavailable"))
    }
    fn runtime_retirement(&self) -> Option<&dyn crate::RuntimeRetirement> {
        None
    }
    /// Resume the same provider object with a reserved generation and admitted service scope.
    async fn resume_runtime(
        &self,
        _request: &RuntimeAdoptionRequest,
        _next: &RuntimeProvisionRequest,
    ) -> Result<RuntimeHandle> {
        Err(unsupported("runtime resumption is unavailable"))
    }
    /// Explicitly replace only the image of an idle, suspended native runtime.
    /// The successor pins its image and retains the original provider and home.
    async fn upgrade_runtime(
        &self,
        _request: &RuntimeAdoptionRequest,
        _next: &RuntimeProvisionRequest,
    ) -> Result<RuntimeHandle> {
        Err(unsupported("native runtime upgrades are unavailable"))
    }
    /// Exactly one creation dispatch; failures cannot be upgraded by name-only recovery.
    async fn provision_runtime_receipted(
        &self,
        _request: &RuntimeProvisionRequest,
        _operation_id: &str,
    ) -> Result<crate::RuntimeProvisionReceipt> {
        Err(unsupported(
            "original provider incarnation receipts are unavailable",
        ))
    }
    /// Delete only the original incarnation and return actual provider effect receipts.
    async fn finalize_runtime(
        &self,
        _original: &crate::RuntimeProvisionReceipt,
        _operation_id: &str,
    ) -> Result<crate::RuntimeFinalityReceipt> {
        Err(unsupported("exact provider finality is unavailable"))
    }
    /// Read the current process incarnation only for a verified persistent home.
    async fn runtime_status(&self, _handle: &RuntimeHandle) -> Result<crate::RuntimeStatus> {
        Err(unsupported("persistent runtime status is not supported"))
    }
    /// Provision an exact non-zero runtime generation.
    async fn provision_runtime(&self, request: &RuntimeProvisionRequest) -> Result<RuntimeHandle> {
        runtime_provision::provision(self, request).await
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
        Err(unsupported("runtime inventory is not supported"))
    }
    /// Attach local runtime state only after proving one exact provider row.
    async fn adopt_runtime(
        &self,
        _request: &RuntimeAdoptionRequest,
    ) -> Result<Option<RuntimeObservation>> {
        Err(unsupported("runtime adoption is not supported"))
    }
    /// Collect adoption and process status within one call. Implementations may
    /// share fresh predicates inside this call; effects still obtain fresh fences.
    async fn inspect_runtime(
        &self,
        request: &RuntimeAdoptionRequest,
    ) -> Result<Option<crate::RuntimeInspection>> {
        runtime_inspection::inspect(self, request).await
    }
    /// Optional independent read preparation; default adapters remain serial.
    async fn inspect_runtime_after(
        &self,
        request: &RuntimeAdoptionRequest,
        admission: &crate::InspectionAdmission,
    ) -> Result<Option<crate::RuntimeInspection>> {
        admission.wait().await?;
        self.inspect_runtime(request).await
    }
    /// Reclaim one exact provider observation without trusting a reused handle.
    async fn reclaim_runtime(&self, _observation: &RuntimeObservation) -> Result<()> {
        Err(unsupported("observed runtime reclamation is not supported"))
    }
    /// Publish an exact owner upload within the retained home, never an arbitrary path.
    async fn upload_agent_attachment(
        &self,
        _handle: &RuntimeHandle,
        _request: &crate::AgentAttachmentRequest,
        _content: Vec<u8>,
    ) -> Result<crate::AgentAttachmentReceipt> {
        Err(unsupported(
            "agent attachments are unavailable on this runtime",
        ))
    }
    /// List one directory in the retained runtime home. Paths are relative to
    /// that home and are validated by the runtime supervisor.
    async fn list_agent_files(
        &self,
        _handle: &RuntimeHandle,
        _request: &crate::AgentFileListRequest,
    ) -> Result<crate::AgentFileListReceipt> {
        Err(unsupported("runtime filesystem access is unavailable"))
    }
    async fn read_agent_file(
        &self,
        _handle: &RuntimeHandle,
        _request: &crate::AgentFileReadRequest,
    ) -> Result<crate::AgentFileReadReceipt> {
        Err(unsupported("runtime filesystem access is unavailable"))
    }
    async fn write_agent_file(
        &self,
        _handle: &RuntimeHandle,
        _request: &crate::AgentFileWriteRequest,
    ) -> Result<crate::AgentFileWriteReceipt> {
        Err(unsupported("runtime filesystem access is unavailable"))
    }

    async fn delete_agent_file(
        &self,
        _handle: &RuntimeHandle,
        _request: &crate::AgentFileDeleteRequest,
    ) -> Result<crate::AgentFileDeleteReceipt> {
        Err(unsupported("runtime filesystem access is unavailable"))
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
        Err(unsupported("restore bootstrap is not supported"))
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
        Err(unsupported(
            "preserving Workspace preparation recovery is unavailable",
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
        Err(unsupported("guarded Workspace removal is unavailable"))
    }
    async fn reconcile_workspace_removal(
        &self,
        _handle: &RuntimeHandle,
        _expected_runtime: &crate::RuntimeStatus,
        _request: &WorkspaceRemoveRequest,
    ) -> Result<crate::WorkspaceRemovalObservation> {
        Err(unsupported(
            "current Workspace removal observation is unavailable",
        ))
    }
    /// Optional ordered resource creation; the caller must prove runtime support.
    /// An error can follow a partial effect. It never authorizes an implicit retry.
    async fn prepare_agent_resources(
        &self,
        _handle: &RuntimeHandle,
        _authority_fence: u64,
        _worktree: &WorktreeRequest,
        _home: &AgentHomeRequest,
    ) -> Result<WorktreeReceipt> {
        Err(unsupported(
            "combined Agent resource preparation is unavailable",
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
        Err(unsupported("skill archive preparation is unavailable"))
    }

    async fn install_agent_skill(
        &self,
        _handle: &RuntimeHandle,
        _authority_fence: u64,
        _request: &crate::AgentSkillInstallRequest,
    ) -> crate::Result<()> {
        Err(unsupported("live skill installation is unavailable"))
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

    /// Reattach only a retained native process with independent state/output proof.
    /// No ordinary reconnect, launch, input or settings fallback is permitted.
    async fn reconnect_agent_for_capacity_recovery(
        &self,
        request: &crate::agent_capacity_recovery::CapacityRecoveryRequest,
        updates: mpsc::Sender<ConversationUpdate>,
    ) -> crate::agent_capacity_recovery::CapacityRecoveryResult<
        crate::agent_capacity_recovery::CapacityRecoveryConnection,
    > {
        request.validate()?;
        let _ = updates;
        Err(crate::agent_capacity_recovery::CapacityRecoveryError::Unsupported)
    }

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

    /// Reattach only to replay one already admitted idle context operation.
    /// The caller must complete that operation before publishing the input.
    #[allow(clippy::too_many_arguments)]
    async fn reconnect_agent_with_context(
        &self,
        handle: &RuntimeHandle,
        authority_fence: u64,
        request: &AgentOpenRequest,
        conversation_id: &str,
        operation_sequence: u64,
        updates: mpsc::Sender<ConversationUpdate>,
    ) -> Result<ConversationChannel> {
        require_admitted_context(operation_sequence)?;
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
        Err(unsupported("agent history export is not supported"))
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
        Err(unsupported("checkpoint writer hold is not supported"))
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
        Err(unsupported("checkpoint writer resume is not supported"))
    }

    async fn restore(&self, handle: &RuntimeHandle, request: &RestoreRequest) -> Result<()>;
    async fn resources(&self, handle: &RuntimeHandle) -> Result<ResourceReading>;

    async fn teardown(&self, handle: &RuntimeHandle) -> Result<()>;
}
