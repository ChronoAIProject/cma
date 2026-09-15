//! Explicit Mongo worker transport simulation. Real filesystem removal is covered by runtime tests.
use layer3_adapter::*;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};
use tokio::sync::mpsc;
pub(crate) struct WorkspaceAdapter {
    inner: Arc<dyn SandboxAdapter>,
    pub calls: AtomicUsize,
    pub lose_response: AtomicBool,
    pub successor: AtomicBool,
    pub unsupported: AtomicBool,
    pub requests: std::sync::Mutex<Vec<WorkspaceRemoveRequest>>,
    pause: std::sync::Mutex<Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>>,
}
impl WorkspaceAdapter {
    pub fn pause_after_removal_once(&self) -> (Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>) {
        let reached = Arc::new(tokio::sync::Notify::new());
        let resume = Arc::new(tokio::sync::Notify::new());
        *self.pause.lock().unwrap() = Some((reached.clone(), resume.clone()));
        (reached, resume)
    }
    pub fn new(inner: Arc<dyn SandboxAdapter>) -> Self {
        Self {
            inner,
            calls: AtomicUsize::new(0),
            lose_response: AtomicBool::new(false),
            successor: AtomicBool::new(false),
            unsupported: AtomicBool::new(false),
            requests: std::sync::Mutex::new(Vec::new()),
            pause: std::sync::Mutex::new(None),
        }
    }
}
#[async_trait::async_trait]
impl SandboxAdapter for WorkspaceAdapter {
    fn capabilities(&self) -> AdapterCapabilities {
        self.inner.capabilities()
    }
    async fn adopt_runtime(
        &self,
        request: &RuntimeAdoptionRequest,
    ) -> Result<Option<RuntimeObservation>> {
        self.inner.adopt_runtime(request).await
    }
    async fn runtime_status(&self, handle: &RuntimeHandle) -> Result<RuntimeStatus> {
        self.inner.runtime_status(handle).await
    }
    async fn runtime_inventory(&self) -> Result<Vec<RuntimeObservation>> {
        self.inner.runtime_inventory().await
    }
    async fn teardown(&self, handle: &RuntimeHandle) -> Result<()> {
        self.inner.teardown(handle).await
    }
    async fn workspace_removal_support(
        &self,
        handle: &RuntimeHandle,
    ) -> Result<WorkspaceRemovalSupport> {
        if self.unsupported.load(Ordering::SeqCst) {
            return Err(AdapterError::Unsupported("fixture old runtime".into()));
        }
        Ok(WorkspaceRemovalSupport {
            runtime: self.inner.runtime_status(handle).await?,
            protocol_version: WORKSPACE_REMOVAL_PROTOCOL,
        })
    }
    async fn reconcile_workspace_removal(
        &self,
        handle: &RuntimeHandle,
        expected: &RuntimeStatus,
        request: &WorkspaceRemoveRequest,
    ) -> Result<WorkspaceRemovalObservation> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.requests.lock().unwrap().push(request.clone());
        let runtime = self.inner.runtime_status(handle).await?;
        if !same_runtime_writer(&runtime, expected) {
            return Err(AdapterError::Rejected("fixture replaced writer".into()));
        }
        if self.lose_response.swap(false, Ordering::SeqCst) {
            return Err(AdapterError::Unavailable(
                "fixture lost current removal response".into(),
            ));
        }
        let pause = self.pause.lock().unwrap().take();
        if let Some((reached, resume)) = pause {
            reached.notify_one();
            resume.notified().await;
        }
        Ok(WorkspaceRemovalObservation {
            runtime,
            receipt: WorkspaceRemoveReceipt {
                operation_id: request.operation_id.clone(),
                request_sha256: request.request_sha256.clone(),
                workspace_id: request.workspace_id.clone(),
            },
            outcome: if self.successor.load(Ordering::SeqCst) {
                WorkspaceRemovalOutcome::SuccessorPresent
            } else {
                WorkspaceRemovalOutcome::CurrentlyAbsent
            },
        })
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
