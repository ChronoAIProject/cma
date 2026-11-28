use super::*;
use crate::sandbox::workspace_worker::{WorkspaceWorker, WorkspaceWorkerDependencies};
use layer2_core::sandbox::store::{SandboxEventStore, WorkspaceStore, WorkspaceWorkerStore};
use layer2_core::sandbox::{
    CreateWorkspace, SandboxEventKind, WorkspaceService, WorkspaceStatus, WorkspaceWorkStep,
};
use layer3_adapter::hosted::{HostedAgentOpenSettings, HostedBackend};
use layer3_adapter::provider::ProviderSandboxRequest;
use layer3_adapter::wire::{AgentOpenReceipt, FencedReceipt, FencedRequest, ResponseFrame};
use layer3_adapter::{AgentLaunch, ConversationChannel, ConversationUpdate, RuntimeObservation};

struct PendingPreparation {
    backend: InMemoryHostedBackend,
    entered: Notify,
    release: Notify,
}

struct BlockingPreparation {
    backend: InMemoryHostedBackend,
    entered: Notify,
    release: Notify,
}

#[async_trait::async_trait]
impl HostedBackend for BlockingPreparation {
    async fn provision(
        &self,
        request: ProviderSandboxRequest,
    ) -> layer3_adapter::Result<RuntimeHandle> {
        self.backend.provision(request).await
    }
    async fn adopt(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
    ) -> layer3_adapter::Result<Option<RuntimeObservation>> {
        self.backend.adopt(handle, request).await
    }
    async fn exchange(
        &self,
        handle: &RuntimeHandle,
        request: RequestFrame,
    ) -> layer3_adapter::Result<ResponseFrame> {
        if matches!(request, RequestFrame::PrepareWorkspace(_)) {
            self.entered.notify_one();
            self.release.notified().await;
        }
        self.backend.exchange(handle, request).await
    }
    async fn open_agent(
        &self,
        handle: &RuntimeHandle,
        launch: FencedRequest<AgentLaunch>,
        resume: Option<String>,
        settings: HostedAgentOpenSettings,
        updates: tokio::sync::mpsc::Sender<ConversationUpdate>,
    ) -> layer3_adapter::Result<(ConversationChannel, FencedReceipt<AgentOpenReceipt>)> {
        self.backend
            .open_agent(handle, launch, resume, settings, updates)
            .await
    }
    async fn teardown(&self, handle: &RuntimeHandle) -> layer3_adapter::Result<()> {
        self.backend.teardown(handle).await
    }
}

#[async_trait::async_trait]
impl HostedBackend for PendingPreparation {
    async fn provision(
        &self,
        request: ProviderSandboxRequest,
    ) -> layer3_adapter::Result<RuntimeHandle> {
        self.backend.provision(request).await
    }
    async fn adopt(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
    ) -> layer3_adapter::Result<Option<RuntimeObservation>> {
        self.backend.adopt(handle, request).await
    }
    async fn exchange(
        &self,
        handle: &RuntimeHandle,
        request: RequestFrame,
    ) -> layer3_adapter::Result<ResponseFrame> {
        let prepare = matches!(request, RequestFrame::PrepareWorkspace(_));
        let result = self.backend.exchange(handle, request).await;
        if prepare {
            self.entered.notify_one();
            self.release.notified().await;
        }
        result
    }
    async fn open_agent(
        &self,
        handle: &RuntimeHandle,
        launch: FencedRequest<AgentLaunch>,
        resume: Option<String>,
        settings: HostedAgentOpenSettings,
        updates: tokio::sync::mpsc::Sender<ConversationUpdate>,
    ) -> layer3_adapter::Result<(ConversationChannel, FencedReceipt<AgentOpenReceipt>)> {
        self.backend
            .open_agent(handle, launch, resume, settings, updates)
            .await
    }
    async fn teardown(&self, handle: &RuntimeHandle) -> layer3_adapter::Result<()> {
        self.backend.teardown(handle).await
    }
}

#[tokio::test(start_paused = true)]
async fn composed_workspace_renews_while_preparation_response_is_pending() {
    let h = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    let parent = h.create().await;
    let now = start_time(&parent);
    h.drive_active(&parent.id, now).await;
    let profiles = Arc::new(InMemoryProfileStore::default());
    let profile = seed_workspace_profile(&profiles, 0).await;
    let service = WorkspaceService::new(h.store.clone(), profiles);
    let mut workspace = service
        .create(
            &caller("owner", [Capability::UseOwnSandboxes]),
            &parent.id,
            CreateWorkspace {
                profile,
                confirmed_digest: None,
            },
        )
        .await
        .unwrap();
    let revision = workspace.revision;
    workspace.work.authority_protocol = Some(WorkAuthorityProtocol::RenewedV1);
    workspace.revision += 1;
    h.store
        .save_workspace(revision, workspace.clone())
        .await
        .unwrap();
    let probe = Arc::new(PendingPreparation {
        backend: h.backend.clone(),
        entered: Notify::new(),
        release: Notify::new(),
    });
    let worker = WorkspaceWorker::new(
        WorkspaceWorkerDependencies {
            store: h.store.clone(),
            adapter: Arc::new(CodexAdapter::new(
                probe.clone(),
                CodexSandboxSpec::default(),
            )),
        },
        WorkspaceWorkerConfig::new("renewed-child", "test")
            .with_execution(Default::default())
            .with_replicas(true),
    );
    let id = workspace.id.clone();
    let running = tokio::spawn(async move { worker.run_one(&id, now).await });
    probe.entered.notified().await;
    let initial = h
        .store
        .get_workspace(&workspace.id)
        .await
        .unwrap()
        .unwrap()
        .work
        .lease
        .unwrap();
    for _ in 0..7 {
        tokio::time::advance(Duration::from_secs(3)).await;
        tokio::task::yield_now().await;
    }
    let extended = h
        .store
        .get_workspace(&workspace.id)
        .await
        .unwrap()
        .unwrap()
        .work
        .lease
        .unwrap();
    assert_eq!(
        (&extended.holder, extended.fence),
        (&initial.holder, initial.fence)
    );
    assert!(extended.expires_at > rfc3339_at(now + Duration::from_secs(21)));
    assert!(h
        .store
        .claim_workspace_work(
            &workspace.id,
            "survivor",
            rfc3339_at(now + Duration::from_secs(21)),
            rfc3339_at(now + Duration::from_secs(36))
        )
        .await
        .unwrap()
        .is_none());
    probe.release.notify_one();
    assert_eq!(running.await.unwrap(), Ok(true));
    let finished = h.store.get_workspace(&workspace.id).await.unwrap().unwrap();
    assert_eq!(finished.work.step, WorkspaceWorkStep::Setup);
    assert!(finished.work.lease.is_none());
    assert_eq!(
        h.backend
            .requests()
            .await
            .iter()
            .filter(|(_, frame)| matches!(frame, RequestFrame::PrepareWorkspace(_)))
            .count(),
        1
    );
}

#[tokio::test(start_paused = true)]
async fn two_instance_workspace_owner_abort_is_taken_over_after_fifteen_seconds_once() {
    let h = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    let (workspace, now) = prepare_workspace(&h).await;
    let probe = Arc::new(BlockingPreparation {
        backend: h.backend.clone(),
        entered: Notify::new(),
        release: Notify::new(),
    });
    let owner = workspace_worker(
        &h,
        "owner-a",
        Arc::new(CodexAdapter::new(
            probe.clone(),
            CodexSandboxSpec::default(),
        )),
    );
    let successor = workspace_worker(
        &h,
        "owner-b",
        Arc::new(CodexAdapter::new(
            Arc::new(h.backend.clone()),
            CodexSandboxSpec::default(),
        )),
    );
    let id = workspace.id.clone();
    let owner_task = tokio::spawn(async move { owner.run_one(&id, now).await });
    probe.entered.notified().await;
    let fault_at = tokio::time::Instant::now();
    owner_task.abort();
    assert!(owner_task.await.unwrap_err().is_cancelled());
    tokio::time::advance(Duration::from_secs(WORK_LEASE_SECONDS)).await;
    tokio::task::yield_now().await;
    assert!(successor
        .run_one(&workspace.id, now + Duration::from_secs(WORK_LEASE_SECONDS))
        .await
        .unwrap());
    let takeover_at = tokio::time::Instant::now();
    assert!(takeover_at - fault_at <= Duration::from_secs(WORK_LEASE_SECONDS));
    let ready = successor
        .run_until_ready(&workspace.id, now + Duration::from_secs(WORK_LEASE_SECONDS))
        .await
        .unwrap()
        .workspace;
    assert_eq!(ready.id, workspace.id);
    assert_eq!(ready.status, WorkspaceStatus::Ready);
    assert!(ready.work.lease.is_none());
    assert!(!successor
        .run_one(&workspace.id, now + Duration::from_secs(WORK_LEASE_SECONDS))
        .await
        .unwrap());
    assert_eq!(
        h.backend
            .requests()
            .await
            .iter()
            .filter(|(_, frame)| matches!(frame, RequestFrame::PrepareWorkspace(_)))
            .count(),
        1
    );
    let events = h
        .store
        .sandbox_events(&workspace.sandbox_id, 0, 50)
        .await
        .unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(&event.kind,
        SandboxEventKind::WorkspaceReady { workspace_id, .. } if workspace_id == &workspace.id))
            .count(),
        1
    );
}

async fn prepare_workspace(h: &Harness) -> (layer2_core::sandbox::Workspace, SystemTime) {
    let parent = h.create().await;
    let now = start_time(&parent);
    h.drive_active(&parent.id, now).await;
    let profiles = Arc::new(InMemoryProfileStore::default());
    let profile = seed_workspace_profile(&profiles, 0).await;
    let service = WorkspaceService::new(h.store.clone(), profiles);
    let mut workspace = service
        .create(
            &caller("owner", [Capability::UseOwnSandboxes]),
            &parent.id,
            CreateWorkspace {
                profile,
                confirmed_digest: None,
            },
        )
        .await
        .unwrap();
    let revision = workspace.revision;
    workspace.work.authority_protocol = Some(WorkAuthorityProtocol::RenewedV1);
    workspace.revision += 1;
    h.store
        .save_workspace(revision, workspace.clone())
        .await
        .unwrap();
    (workspace, now)
}

fn workspace_worker(
    h: &Harness,
    holder: &str,
    adapter: Arc<dyn SandboxAdapter>,
) -> Arc<WorkspaceWorker> {
    WorkspaceWorker::new(
        WorkspaceWorkerDependencies {
            store: h.store.clone(),
            adapter,
        },
        WorkspaceWorkerConfig::new(holder, "test")
            .with_execution(Default::default())
            .with_replicas(true),
    )
}
