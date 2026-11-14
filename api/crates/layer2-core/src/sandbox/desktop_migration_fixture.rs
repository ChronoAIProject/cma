use super::*;
use crate::sandbox::store::{
    AgentStore, InMemorySandboxStore, SandboxStore, WorkspaceStore, WorktreeStore,
};
use crate::sandbox::{
    AgentWorkStep, Sandbox, SandboxResourceReadError, SandboxResourceReader, SandboxResourceReading,
};
use crate::{Authorization, Capability, SubjectId};
use layer3_adapter::*;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tokio::sync::{mpsc, Notify};

#[derive(Default)]
pub(super) struct Probe {
    pub entered: Notify,
    pub release: Notify,
    pub calls: AtomicUsize,
    pub sockets: Arc<AtomicUsize>,
    pub fail: AtomicBool,
}
struct Socket(Arc<AtomicUsize>);
impl Drop for Socket {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}
#[async_trait::async_trait]
impl layer3_adapter::DesktopConnection for Socket {
    async fn recv(&mut self) -> layer3_adapter::Result<Option<DesktopFrame>> {
        Ok(None)
    }
    async fn send(&mut self, _: DesktopFrame) -> layer3_adapter::Result<()> {
        Ok(())
    }
}
struct Resources;
#[async_trait::async_trait]
impl SandboxResourceReader for Resources {
    async fn sandbox_resources(
        &self,
        _: &Sandbox,
    ) -> std::result::Result<SandboxResourceReading, SandboxResourceReadError> {
        panic!("desktop does not query resources")
    }
}

pub(super) struct Fixture {
    pub service: Arc<DesktopService>,
    pub store: Arc<InMemorySandboxStore>,
    pub probe: Arc<Probe>,
    pub id: SandboxId,
    pub agent_id: AgentId,
    pub owner: Caller,
}
impl Fixture {
    pub async fn new() -> Self {
        let store = Arc::new(InMemorySandboxStore::default());
        let mut sandbox = crate::sandbox::tests::sandbox();
        sandbox.id = SandboxId::generate();
        sandbox
            .profile
            .content
            .environment_variables
            .push(crate::profiles::EnvironmentVariable {
                name: "CMA_MACHINE_INCLUDED".into(),
                value: "1".into(),
            });
        sandbox.runtime = Some(RuntimeBinding {
            boot_id: None,
            runtime_id: "runtime-desktop".into(),
            runtime_version: "runtime-image".into(),
            provision_generation: Some(1),
            bound_at: crate::now_rfc3339(),
        });
        let id = sandbox.id.clone();
        store.create_sandbox(sandbox).await.unwrap();
        let mut workspace = crate::sandbox::tests::workspace();
        workspace.sandbox_id = id.clone();
        store.create_workspace(workspace).await.unwrap();
        let mut worktree = crate::sandbox::tests::worktree();
        worktree.sandbox_id = id.clone();
        store.create_worktree(worktree).await.unwrap();
        let mut agent = crate::sandbox::tests::agent();
        agent.sandbox_id = id.clone();
        agent.status = AgentStatus::Ready;
        agent.work.step = AgentWorkStep::None;
        let agent_id = agent.id.clone();
        store.create_agent(agent).await.unwrap();
        let probe = Arc::new(Probe::default());
        let service = Arc::new(DesktopService::new(
            AgentReadService::new(store.clone(), Arc::new(Resources)).with_sessions(store.clone()),
            probe.clone(),
        ));
        Self {
            service,
            store,
            probe,
            id,
            agent_id,
            owner: Caller::new(
                SubjectId("owner".into()),
                Authorization::new([Capability::UseOwnSandboxes]),
            ),
        }
    }
    pub fn start(
        &self,
    ) -> tokio::task::JoinHandle<std::result::Result<super::DesktopConnection, DesktopError>> {
        let (service, owner, id) = (
            self.service.clone(),
            self.owner.clone(),
            self.agent_id.clone(),
        );
        tokio::spawn(async move { service.open(&owner, &id).await })
    }
    pub async fn entered(&self) {
        tokio::time::timeout(Duration::from_secs(2), self.probe.entered.notified())
            .await
            .unwrap();
    }
}
#[async_trait::async_trait]
impl SandboxAdapter for Probe {
    async fn open_desktop(
        &self,
        _: &RuntimeHandle,
    ) -> layer3_adapter::Result<DesktopConnectionHandle> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.entered.notify_one();
        self.release.notified().await;
        if self.fail.load(Ordering::SeqCst) {
            return Err(AdapterError::Unavailable("held open failed".into()));
        }
        self.sockets.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(Socket(self.sockets.clone())))
    }
    fn capabilities(&self) -> AdapterCapabilities {
        panic!("unexpected runtime effect")
    }
    async fn provision(&self, _spec: &SandboxSpec) -> Result<RuntimeHandle> {
        panic!("unexpected runtime effect")
    }
    async fn run_install(
        &self,
        _handle: &RuntimeHandle,
        _request: &InstallRequest,
    ) -> Result<InstallReceipt> {
        panic!("unexpected runtime effect")
    }
    async fn deliver_credential(
        &self,
        _handle: &RuntimeHandle,
        _frame: CredentialFrame,
    ) -> Result<()> {
        panic!("unexpected runtime effect")
    }
    async fn prepare_workspace(
        &self,
        _handle: &RuntimeHandle,
        _request: &WorkspaceRequest,
    ) -> Result<WorkspaceReceipt> {
        panic!("unexpected runtime effect")
    }
    async fn run_workspace_setup(
        &self,
        _handle: &RuntimeHandle,
        _request: &WorkspaceSetupRequest,
    ) -> Result<SetupReceipt> {
        panic!("unexpected runtime effect")
    }
    async fn remove_workspace(
        &self,
        _handle: &RuntimeHandle,
        _request: &WorkspaceRemoveRequest,
    ) -> Result<()> {
        panic!("unexpected runtime effect")
    }
    async fn add_worktree(
        &self,
        _handle: &RuntimeHandle,
        _authority_fence: u64,
        _request: &WorktreeRequest,
    ) -> Result<WorktreeReceipt> {
        panic!("unexpected runtime effect")
    }
    async fn remove_worktree(
        &self,
        _handle: &RuntimeHandle,
        _authority_fence: u64,
        _request: &WorktreeRef,
    ) -> Result<()> {
        panic!("unexpected runtime effect")
    }
    async fn materialize_agent_home(
        &self,
        _handle: &RuntimeHandle,
        _authority_fence: u64,
        _request: &AgentHomeRequest,
    ) -> Result<()> {
        panic!("unexpected runtime effect")
    }
    async fn remove_agent_home(
        &self,
        _handle: &RuntimeHandle,
        _authority_fence: u64,
        _request: &AgentHomeRemoveRequest,
    ) -> Result<()> {
        panic!("unexpected runtime effect")
    }
    async fn open_agent(
        &self,
        _handle: &RuntimeHandle,
        _authority_fence: u64,
        _request: &AgentOpenRequest,
        _updates: mpsc::Sender<ConversationUpdate>,
    ) -> Result<ConversationChannel> {
        panic!("unexpected runtime effect")
    }
    async fn reconnect_agent(
        &self,
        _handle: &RuntimeHandle,
        _authority_fence: u64,
        _request: &AgentOpenRequest,
        _conversation_id: &str,
        _updates: mpsc::Sender<ConversationUpdate>,
    ) -> Result<ConversationChannel> {
        panic!("unexpected runtime effect")
    }
    async fn close_agent(
        &self,
        _handle: &RuntimeHandle,
        _authority_fence: u64,
        _request: &AgentCloseRequest,
    ) -> Result<()> {
        panic!("unexpected runtime effect")
    }
    async fn capture_agent(
        &self,
        _handle: &RuntimeHandle,
        _agent: &AgentSlot,
    ) -> Result<ConversationSnapshot> {
        panic!("unexpected runtime effect")
    }
    async fn checkpoint(
        &self,
        _handle: &RuntimeHandle,
        _request: &CheckpointRequest,
    ) -> Result<CheckpointReceipt> {
        panic!("unexpected runtime effect")
    }
    async fn restore(&self, _handle: &RuntimeHandle, _request: &RestoreRequest) -> Result<()> {
        panic!("unexpected runtime effect")
    }
    async fn resources(&self, _handle: &RuntimeHandle) -> Result<ResourceReading> {
        panic!("unexpected runtime effect")
    }
    async fn teardown(&self, _handle: &RuntimeHandle) -> Result<()> {
        panic!("unexpected runtime effect")
    }
}
