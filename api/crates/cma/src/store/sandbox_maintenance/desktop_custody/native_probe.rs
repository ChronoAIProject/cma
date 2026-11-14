//! Real desktop service probe for migration custody lifetime regressions.
use layer2_core::profiles::EnvironmentVariable;
use layer2_core::sandbox::store::{
    AgentStore, InMemorySandboxStore, SandboxSessionStore, SandboxStore, WorkspaceStore,
    WorktreeStore,
};
use layer2_core::sandbox::{
    runtime_desktop_activity_snapshot, AgentReadService, DesktopError, DesktopService, Sandbox,
    SandboxResourceReadError, SandboxResourceReader, SandboxResourceReading, Worktree,
    WorktreeStatus,
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
        let service = Arc::new(DesktopService::new(
            AgentReadService::new(store.clone(), Arc::new(Resources)).with_sessions(store.clone()),
            probe.clone(),
        ));
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
        let result = tokio::time::timeout(WAIT, self.service.open(&self.owner, &self.agent_id))
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
        let opening = tokio::spawn(async move { service.open(&owner, &id).await });
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
        self.assert_session(false).await;
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
    async fn open_desktop(
        &self,
        handle: &RuntimeHandle,
    ) -> layer3_adapter::Result<DesktopConnectionHandle> {
        assert_eq!(handle.as_str(), self.runtime_id);
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
mod regressions {
    use super::super::{run_with_registry, Registry};
    use super::*;
    use layer2_core::sandbox::store::SandboxStoreError;
    use tokio::sync::oneshot;

    struct Held {
        caller: tokio::task::JoinHandle<std::result::Result<(), SandboxStoreError>>,
        entered: oneshot::Receiver<()>,
        commit: oneshot::Sender<()>,
        reconciling: oneshot::Receiver<()>,
        terminal: oneshot::Sender<()>,
        finished: oneshot::Receiver<()>,
    }

    fn start(id: SandboxId, registry: Arc<Registry>, refuse: bool) -> Held {
        let (entered_tx, entered) = oneshot::channel();
        let (commit, commit_rx) = oneshot::channel();
        let (reconciling_tx, reconciling) = oneshot::channel();
        let (terminal, terminal_rx) = oneshot::channel();
        let (finished_tx, finished) = oneshot::channel();
        let caller = tokio::spawn(async move {
            run_with_registry(
                registry,
                &id,
                Duration::from_secs(30),
                move |mut custody| async move {
                    let commit_and_reconcile = async move {
                        custody.commit_started();
                        entered_tx.send(()).unwrap();
                        commit_rx.await.unwrap();
                        let watermark = mongodb::bson::Timestamp {
                            time: 1,
                            increment: 1,
                        };
                        custody
                            .reconcile(Some(watermark), move |at| async move {
                                assert_eq!(at, watermark);
                                reconciling_tx.send(()).unwrap();
                                terminal_rx.await.unwrap();
                                Ok(true)
                            })
                            .await
                            .unwrap();
                        drop(custody);
                        finished_tx.send(()).unwrap();
                        Ok::<(), SandboxStoreError>(())
                    };
                    if refuse {
                        // A refused response cannot reclaim custody already moved
                        // to a detached commit/reconciliation continuation.
                        drop(tokio::spawn(commit_and_reconcile));
                        Err(SandboxStoreError::Conflict)
                    } else {
                        commit_and_reconcile.await
                    }
                },
            )
            .await
        });
        Held {
            caller,
            entered,
            commit,
            reconciling,
            terminal,
            finished,
        }
    }

    async fn reached(signal: oneshot::Receiver<()>) {
        tokio::time::timeout(WAIT, signal)
            .await
            .expect("owned phase reached")
            .unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn desktop_stays_excluded_through_detached_commit_and_reconciliation() {
        for exit in ["timeout", "cancel", "refusal"] {
            let fixture = Fixture::new().await;
            let held = start(
                fixture.id.clone(),
                Arc::new(Registry::new(2)),
                exit == "refusal",
            );
            reached(held.entered).await;
            fixture.assert_blocked().await;
            match exit {
                "timeout" => {
                    tokio::time::advance(Duration::from_secs(31)).await;
                    assert!(held.caller.await.unwrap().is_err());
                }
                "cancel" => {
                    held.caller.abort();
                    assert!(held.caller.await.unwrap_err().is_cancelled());
                }
                "refusal" => assert!(matches!(
                    held.caller.await.unwrap(),
                    Err(SandboxStoreError::Conflict)
                )),
                _ => unreachable!(),
            }
            // The caller has ended, but the original commit is still pending.
            fixture.assert_blocked().await;
            held.commit.send(()).unwrap();
            reached(held.reconciling).await;
            fixture.assert_blocked().await;
            held.terminal.send(()).unwrap();
            reached(held.finished).await;
            fixture.assert_native_open().await;
        }
    }

    #[tokio::test(start_paused = true)]
    async fn capacity_refusal_cannot_start_effects_or_release_existing_desktop_custody() {
        let first = Fixture::new().await;
        let second = Fixture::new().await;
        let registry = Arc::new(Registry::new(1));
        let held = start(first.id.clone(), registry.clone(), false);
        reached(held.entered).await;
        let calls = Arc::new(AtomicUsize::new(0));
        let attempted = calls.clone();
        let result = run_with_registry(registry, &second.id, WAIT, move |_| {
            attempted.fetch_add(1, Ordering::SeqCst);
            async { Ok(()) }
        })
        .await;
        assert!(result.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        first.assert_blocked().await;
        second.assert_native_open().await;
        held.caller.abort();
        assert!(held.caller.await.unwrap_err().is_cancelled());
        held.commit.send(()).unwrap();
        reached(held.reconciling).await;
        first.assert_blocked().await;
        held.terminal.send(()).unwrap();
        reached(held.finished).await;
        first.assert_native_open().await;
    }
}
