use layer3_adapter::*;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};
use tokio::sync::mpsc;
#[path = "finalization_provider.rs"]
mod finality;
pub(crate) struct RetirementAdapter {
    inner: Arc<dyn SandboxAdapter>,
    pub teardowns: AtomicUsize,
    pub lose_teardown: AtomicBool,
    pub inventory_unavailable: AtomicBool,
    proof: Option<finality::ProviderFixture>,
}
impl RetirementAdapter {
    pub fn new(inner: Arc<dyn SandboxAdapter>) -> Self {
        Self {
            inner,
            teardowns: AtomicUsize::new(0),
            lose_teardown: AtomicBool::new(false),
            inventory_unavailable: AtomicBool::new(false),
            proof: None,
        }
    }
    pub fn with_finality(mut self) -> Self {
        self.proof = Some(finality::ProviderFixture::default());
        self
    }
}
#[async_trait::async_trait]
impl SandboxAdapter for RetirementAdapter {
    async fn provision_runtime(&self, request: &RuntimeProvisionRequest) -> Result<RuntimeHandle> {
        self.inner.provision_runtime(request).await
    }
    async fn provision_runtime_receipted(
        &self,
        request: &RuntimeProvisionRequest,
        operation_id: &str,
    ) -> Result<RuntimeProvisionReceipt> {
        match &self.proof {
            Some(proof) => {
                proof
                    .provision(self.inner.as_ref(), request, operation_id)
                    .await
            }
            None => {
                self.inner
                    .provision_runtime_receipted(request, operation_id)
                    .await
            }
        }
    }
    async fn finalize_runtime(
        &self,
        original: &RuntimeProvisionReceipt,
        operation_id: &str,
    ) -> Result<RuntimeFinalityReceipt> {
        self.teardowns.fetch_add(1, Ordering::SeqCst);
        match &self.proof {
            Some(proof) => {
                proof
                    .finalize(self.inner.as_ref(), original, operation_id)
                    .await
            }
            None => self.inner.finalize_runtime(original, operation_id).await,
        }
    }
    fn capabilities(&self) -> AdapterCapabilities {
        self.inner.capabilities()
    }
    fn home_volumes(&self) -> Option<&dyn HomeVolumeStore> {
        self.inner.home_volumes()
    }
    async fn runtime_status(&self, handle: &RuntimeHandle) -> Result<RuntimeStatus> {
        self.inner.runtime_status(handle).await
    }
    async fn adopt_runtime(
        &self,
        request: &RuntimeAdoptionRequest,
    ) -> Result<Option<RuntimeObservation>> {
        self.inner.adopt_runtime(request).await
    }
    async fn runtime_inventory(&self) -> Result<Vec<RuntimeObservation>> {
        if self.inventory_unavailable.load(Ordering::SeqCst) {
            return Err(AdapterError::Unavailable(
                "fixture inventory unavailable".into(),
            ));
        }
        self.inner.runtime_inventory().await
    }
    async fn teardown(&self, handle: &RuntimeHandle) -> Result<()> {
        self.teardowns.fetch_add(1, Ordering::SeqCst);
        self.inner.teardown(handle).await?;
        if self.lose_teardown.swap(false, Ordering::SeqCst) {
            Err(AdapterError::Unavailable(
                "fixture lost teardown response".into(),
            ))
        } else {
            Ok(())
        }
    }
    async fn provision(&self, spec: &SandboxSpec) -> Result<RuntimeHandle> {
        self.inner.provision(spec).await
    }
    async fn run_install(
        &self,
        handle: &RuntimeHandle,
        request: &InstallRequest,
    ) -> Result<InstallReceipt> {
        self.inner.run_install(handle, request).await
    }
    async fn deliver_credential(
        &self,
        handle: &RuntimeHandle,
        frame: CredentialFrame,
    ) -> Result<()> {
        self.inner.deliver_credential(handle, frame).await
    }
    async fn prepare_workspace(
        &self,
        handle: &RuntimeHandle,
        request: &WorkspaceRequest,
    ) -> Result<WorkspaceReceipt> {
        self.inner.prepare_workspace(handle, request).await
    }
    async fn run_workspace_setup(
        &self,
        handle: &RuntimeHandle,
        request: &WorkspaceSetupRequest,
    ) -> Result<SetupReceipt> {
        self.inner.run_workspace_setup(handle, request).await
    }
    async fn remove_workspace(
        &self,
        handle: &RuntimeHandle,
        request: &WorkspaceRemoveRequest,
    ) -> Result<()> {
        self.inner.remove_workspace(handle, request).await
    }
    async fn add_worktree(
        &self,
        handle: &RuntimeHandle,
        authority_fence: u64,
        request: &WorktreeRequest,
    ) -> Result<WorktreeReceipt> {
        self.inner
            .add_worktree(handle, authority_fence, request)
            .await
    }
    async fn remove_worktree(
        &self,
        handle: &RuntimeHandle,
        authority_fence: u64,
        request: &WorktreeRef,
    ) -> Result<()> {
        self.inner
            .remove_worktree(handle, authority_fence, request)
            .await
    }
    async fn materialize_agent_home(
        &self,
        handle: &RuntimeHandle,
        authority_fence: u64,
        request: &AgentHomeRequest,
    ) -> Result<()> {
        self.inner
            .materialize_agent_home(handle, authority_fence, request)
            .await
    }
    async fn remove_agent_home(
        &self,
        handle: &RuntimeHandle,
        authority_fence: u64,
        request: &AgentHomeRemoveRequest,
    ) -> Result<()> {
        self.inner
            .remove_agent_home(handle, authority_fence, request)
            .await
    }
    async fn open_agent(
        &self,
        handle: &RuntimeHandle,
        authority_fence: u64,
        request: &AgentOpenRequest,
        updates: mpsc::Sender<ConversationUpdate>,
    ) -> Result<ConversationChannel> {
        self.inner
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
    ) -> Result<ConversationChannel> {
        self.inner
            .reconnect_agent(handle, authority_fence, request, conversation_id, updates)
            .await
    }
    async fn close_agent(
        &self,
        handle: &RuntimeHandle,
        authority_fence: u64,
        request: &AgentCloseRequest,
    ) -> Result<()> {
        self.inner
            .close_agent(handle, authority_fence, request)
            .await
    }
    async fn capture_agent(
        &self,
        handle: &RuntimeHandle,
        agent: &AgentSlot,
    ) -> Result<ConversationSnapshot> {
        self.inner.capture_agent(handle, agent).await
    }
    async fn checkpoint(
        &self,
        handle: &RuntimeHandle,
        request: &CheckpointRequest,
    ) -> Result<CheckpointReceipt> {
        self.inner.checkpoint(handle, request).await
    }
    async fn restore(&self, handle: &RuntimeHandle, request: &RestoreRequest) -> Result<()> {
        self.inner.restore(handle, request).await
    }
    async fn resources(&self, handle: &RuntimeHandle) -> Result<ResourceReading> {
        self.inner.resources(handle).await
    }
}
