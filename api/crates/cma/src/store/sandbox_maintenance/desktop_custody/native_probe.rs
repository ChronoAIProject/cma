//! Real desktop service probe for migration custody lifetime regressions.
use layer2_core::profiles::EnvironmentVariable;
use layer2_core::sandbox::store::{
    AgentStore, DesktopSessionRequest, InMemorySandboxStore, SandboxSessionStore, SandboxStore,
    SandboxStoreError, WorkspaceStore, WorktreeStore,
};
use layer2_core::sandbox::{
    runtime_desktop_activity_snapshot, AgentReadService, DesktopAdmission, DesktopAuthorization,
    DesktopError, DesktopService, Sandbox, SandboxResourceReadError, SandboxResourceReader,
    SandboxResourceReading, Worktree, WorktreeStatus,
};
use layer2_core::{AgentId, Authorization, Caller, Capability, SandboxId};
use layer3_adapter::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, Notify};

const WAIT: Duration = Duration::from_secs(2);

#[derive(Default)]
struct Probe {
    runtime_id: String,
    entered: Notify,
    release: Notify,
    calls: AtomicUsize,
    sockets: Arc<AtomicUsize>,
}
struct Socket(Arc<AtomicUsize>);
struct Control;
#[async_trait::async_trait]
impl DesktopOwnershipControl for Control {
    async fn revoke_ownership(&self, _: &DesktopOwnership) -> layer3_adapter::Result<()> {
        Ok(())
    }
}
struct CurrentAuthorization;
#[async_trait::async_trait]
impl DesktopAuthorization for CurrentAuthorization {
    async fn current(&self, _: &Caller) -> std::result::Result<(), DesktopError> {
        Ok(())
    }
}
struct Sessions(Arc<InMemorySandboxStore>);
#[async_trait::async_trait]
impl SandboxSessionStore for Sessions {
    async fn acquire_exclusive_sandbox_session(
        &self,
        request: &DesktopSessionRequest,
    ) -> std::result::Result<DesktopOwnership, SandboxStoreError> {
        let current = self
            .0
            .get_sandbox(&request.sandbox_id)
            .await?
            .ok_or(SandboxStoreError::NotFound)?;
        if current.runtime.as_ref() != Some(&request.runtime) {
            return Err(SandboxStoreError::Conflict);
        }
        let now = std::time::SystemTime::now();
        let issued = now
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        self.open_sandbox_session(
            &request.sandbox_id,
            &request.session_id,
            &layer2_core::rfc3339_at(now + Duration::from_secs(10)),
        )
        .await?;
        Ok(DesktopOwnership {
            sandbox_id: request.sandbox_id.to_string(),
            session_id: request.session_id.clone(),
            runtime_id: request.runtime.runtime_id.clone(),
            boot_id: request.runtime.boot_id.clone(),
            epoch: 1,
            issued_at_ms: issued,
            expires_at_ms: issued + 10_000,
        })
    }

    async fn release_exclusive_sandbox_session(
        &self,
        claim: &DesktopOwnership,
    ) -> std::result::Result<(), SandboxStoreError> {
        self.close_sandbox_session(&SandboxId(claim.sandbox_id.clone()), &claim.session_id)
            .await
    }

    async fn open_sandbox_session(
        &self,
        id: &SandboxId,
        session: &str,
        expiry: &str,
    ) -> std::result::Result<(), SandboxStoreError> {
        self.0.open_sandbox_session(id, session, expiry).await
    }
    async fn renew_sandbox_session(
        &self,
        id: &SandboxId,
        session: &str,
        expiry: &str,
    ) -> std::result::Result<(), SandboxStoreError> {
        self.0.renew_sandbox_session(id, session, expiry).await
    }
    async fn close_sandbox_session(
        &self,
        id: &SandboxId,
        session: &str,
    ) -> std::result::Result<(), SandboxStoreError> {
        self.0.close_sandbox_session(id, session).await
    }
    async fn has_open_sandbox_session(
        &self,
        id: &SandboxId,
        at: &str,
    ) -> std::result::Result<bool, SandboxStoreError> {
        self.0.has_open_sandbox_session(id, at).await
    }
}
impl Drop for Socket {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}
#[async_trait::async_trait]
impl layer3_adapter::DesktopConnection for Socket {
    async fn recv(&mut self) -> layer3_adapter::Result<Option<DesktopFrame>> {
        panic!("unexpected desktop frame read")
    }
    async fn send(&mut self, _: DesktopFrame) -> layer3_adapter::Result<()> {
        panic!("unexpected desktop frame write")
    }
    fn ownership_control(&self) -> Option<Arc<dyn DesktopOwnershipControl>> {
        Some(Arc::new(Control))
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
    pub(super) id: SandboxId,
    service: Arc<DesktopService>,
    store: Arc<InMemorySandboxStore>,
    probe: Arc<Probe>,
    owner: Caller,
    agent_id: AgentId,
}
impl Fixture {
    pub(super) async fn new() -> Self {
        use crate::store::sandbox_maintenance::mongo_integration_tests::{
            active_sandbox, ready_agent,
        };
        let id = SandboxId::generate();
        let mut sandbox = active_sandbox(id.as_str());
        sandbox
            .profile
            .content
            .environment_variables
            .push(EnvironmentVariable {
                name: "CMA_MACHINE_INCLUDED".into(),
                value: "1".into(),
            });
        let agent = ready_agent(&sandbox, AgentId::generate().as_str());
        let workspace = crate::store::sandbox_mongo_integration_tests::ready_workspace(
            &sandbox,
            agent.workspace_id.as_str(),
            "desktop-custody",
        );
        let worktree = Worktree {
            id: agent.worktree_id.clone(),
            workspace_id: workspace.id.clone(),
            sandbox_id: id.clone(),
            owner: sandbox.owner.clone(),
            name: "wt-1".into(),
            path: format!("{}/.worktrees/wt-1", workspace.path),
            branch: "cma/wt-1".into(),
            base_commit: workspace.resolved_commit.clone().unwrap(),
            status: WorktreeStatus::Ready,
            agent_id: Some(agent.id.clone()),
            revision: 1,
        };
        sandbox.validate().unwrap();
        workspace.validate_prerequisites(&sandbox).unwrap();
        worktree.validate_prerequisites(&workspace).unwrap();
        agent.validate_prerequisites(&worktree).unwrap();
        let owner = Caller::new(
            sandbox.owner.clone(),
            Authorization::new([Capability::UseOwnSandboxes]),
        );
        let agent_id = agent.id.clone();
        let store = Arc::new(InMemorySandboxStore::default());
        store.create_sandbox(sandbox).await.unwrap();
        store.create_workspace(workspace).await.unwrap();
        store.create_worktree(worktree).await.unwrap();
        store.create_agent(agent).await.unwrap();
        let probe = Arc::new(Probe {
            runtime_id: format!("runtime-{id}"),
            ..Probe::default()
        });
        let service = Arc::new(
            DesktopService::new(
                AgentReadService::new(store.clone(), Arc::new(Resources))
                    .with_sessions(Arc::new(Sessions(store.clone()))),
                probe.clone(),
            )
            .with_admission(DesktopAdmission::Protected)
            .with_runtime_migration_tracking(true),
        );
        service
            .available(&owner, &agent_id)
            .await
            .expect("eligible desktop lineage");
        Self {
            id,
            service,
            store,
            probe,
            owner,
            agent_id,
        }
    }

    pub(super) async fn assert_blocked(&self) {
        let before = self.probe.calls.load(Ordering::SeqCst);
        let result = tokio::time::timeout(
            WAIT,
            self.service.open_authorized(
                &self.owner,
                &self.agent_id,
                Arc::new(CurrentAuthorization),
            ),
        )
        .await
        .expect("desktop refusal must not wait on native open");
        assert!(matches!(result, Err(DesktopError::Busy)));
        assert_eq!(self.probe.calls.load(Ordering::SeqCst), before);
        assert_eq!(self.probe.sockets.load(Ordering::SeqCst), 0);
        self.assert_session(false).await;
    }

    pub(super) async fn assert_native_open(&self) {
        let before = self.probe.calls.load(Ordering::SeqCst);
        let (service, owner, id) = (
            self.service.clone(),
            self.owner.clone(),
            self.agent_id.clone(),
        );
        let opening = tokio::spawn(async move {
            service
                .open_authorized(&owner, &id, Arc::new(CurrentAuthorization))
                .await
        });
        tokio::time::timeout(WAIT, self.probe.entered.notified())
            .await
            .expect("native open entered");
        assert_eq!(self.probe.calls.load(Ordering::SeqCst), before + 1);
        assert_eq!(
            runtime_desktop_activity_snapshot(&self.id)
                .unwrap()
                .opening_or_open,
            1
        );
        self.assert_session(true).await;
        self.probe.release.notify_one();
        let mut connection = tokio::time::timeout(WAIT, opening)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(self.probe.sockets.load(Ordering::SeqCst), 1);
        self.assert_session(true).await;
        self.service.close_connection(&mut connection).await;
        drop(connection);
        assert_eq!(self.probe.sockets.load(Ordering::SeqCst), 0);
        assert_eq!(
            runtime_desktop_activity_snapshot(&self.id)
                .unwrap()
                .opening_or_open,
            0
        );
        self.assert_session(false).await;
    }

    async fn assert_session(&self, expected: bool) {
        assert_eq!(
            self.store
                .has_open_sandbox_session(&self.id, &layer2_core::now_rfc3339())
                .await
                .unwrap(),
            expected
        );
    }
}

#[async_trait::async_trait]
impl SandboxAdapter for Probe {
    async fn desktop_ownership_challenge(
        &self,
        _: &RuntimeHandle,
    ) -> Result<DesktopOwnershipChallenge> {
        Ok(DesktopOwnershipChallenge {
            nonce: "fixture-challenge".into(),
            valid_for_ms: 10_000,
        })
    }

    async fn open_desktop_owned(
        &self,
        handle: &RuntimeHandle,
        claim: &DesktopOwnership,
        _: &DesktopOwnershipChallenge,
    ) -> layer3_adapter::Result<DesktopConnectionHandle> {
        assert_eq!(handle.as_str(), self.runtime_id);
        assert_eq!(claim.runtime_id, self.runtime_id);
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.entered.notify_one();
        self.release.notified().await;
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

#[cfg(test)]
mod regressions;
