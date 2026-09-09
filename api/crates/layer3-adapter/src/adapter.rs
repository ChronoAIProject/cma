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
    /// Abilities proved for the admitted runtime contract.
    fn capabilities(&self) -> AdapterCapabilities;

    /// Optional durable storage authority; absence must never fall back to an empty home.
    fn home_volumes(&self) -> Option<&dyn crate::HomeVolumeStore> {
        None
    }

    async fn provision(&self, spec: &SandboxSpec) -> Result<RuntimeHandle>;

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

    /// Reclaim one exact provider observation without trusting a reused handle.
    async fn reclaim_runtime(&self, _observation: &RuntimeObservation) -> Result<()> {
        Err(crate::AdapterError::Unsupported(
            "observed runtime reclamation is not supported".into(),
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

    async fn remove_workspace(
        &self,
        handle: &RuntimeHandle,
        request: &WorkspaceRemoveRequest,
    ) -> Result<()>;

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
