use super::*;
use crate::sandbox::workspace_worker::{
    WorkspaceWorker, WorkspaceWorkerConfig, WorkspaceWorkerDependencies,
};
use layer2_core::sandbox::store::WorkspaceStore;
use layer2_core::sandbox::{
    CreateInlineWorkspace, InlineWorkspaceSource, ProvisioningExecution, TypedFailure, Workspace,
    WorkspaceService, WorkspaceStatus, WorkspaceWorkStep,
};
use layer3_adapter::hosted::{HostedAgentOpenSettings, HostedBackend};
use layer3_adapter::provider::ProviderSandboxRequest;
use layer3_adapter::wire::{AgentOpenReceipt, FencedReceipt, FencedRequest, ResponseFrame};
use layer3_adapter::{
    AdapterError, AgentLaunch, ConversationChannel, ConversationUpdate, RuntimeObservation,
    WorkspacePrepareRecoveryReceipt, WorkspacePrepareRecoveryRequest,
};

struct RecoveryProbe {
    backend: InMemoryHostedBackend,
    requests: Mutex<Vec<WorkspacePrepareRecoveryRequest>>,
    rejection: Mutex<Option<AdapterError>>,
    lose_response: AtomicBool,
}
#[async_trait::async_trait]
impl HostedBackend for RecoveryProbe {
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
        frame: RequestFrame,
    ) -> layer3_adapter::Result<ResponseFrame> {
        let RequestFrame::RecoverWorkspacePrepare(frame) = frame else {
            return self.backend.exchange(handle, frame).await;
        };
        self.requests.lock().unwrap().push(frame.request.clone());
        if let Some(error) = self.rejection.lock().unwrap().take() {
            return Err(error);
        }
        let request = frame.request;
        let ResponseFrame::PrepareWorkspace(mut prepared) = self
            .backend
            .exchange(
                handle,
                RequestFrame::PrepareWorkspace(request.original.clone()),
            )
            .await?
        else {
            panic!("expected real preparation receipt")
        };
        if self.lose_response.swap(false, Ordering::SeqCst) {
            return Err(AdapterError::Unavailable(
                "response lost after effect".into(),
            ));
        }
        prepared.operation_id = request.operation_id.clone();
        prepared.request_sha256 = request.request_sha256.clone();
        Ok(ResponseFrame::RecoverWorkspacePrepare(
            WorkspacePrepareRecoveryReceipt {
                prepared,
                preserved_path: request.preserved_path(),
                original_journal_sha256: "a".repeat(64),
            },
        ))
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
struct Fixture {
    harness: Harness,
    service: Arc<WorkspaceService>,
    original: Workspace,
    now: SystemTime,
    probe: Arc<RecoveryProbe>,
    homes: Arc<home_tests::Homes>,
}
impl Fixture {
    async fn new() -> Self {
        let harness = Harness::new(vec![], 0, Duration::from_secs(60)).await;
        let parent = harness.create().await;
        let now = start_time(&parent) + Duration::from_secs(60);
        let homes = home_tests::Homes::new(&harness);
        let provisioner = home_tests::worker(&harness, homes.clone(), "original-parent");
        for _ in 0..8 {
            if harness.stored(&parent.id).await.status == SandboxStatus::Active {
                break;
            }
            provisioner.run_one(&parent.id, now).await.unwrap();
        }
        assert_eq!(
            harness.stored(&parent.id).await.status,
            SandboxStatus::Active
        );
        let service = WorkspaceService::new(
            harness.store.clone(),
            Arc::new(InMemoryProfileStore::default()),
        );
        let mut original = service
            .create_inline(
                &caller("owner", [Capability::UseOwnSandboxes]),
                &parent.id,
                CreateInlineWorkspace {
                    source: InlineWorkspaceSource::Empty,
                    name: Some("original".into()),
                },
            )
            .await
            .unwrap();
        let revision = original.revision;
        original.work.execution = Some(ProvisioningExecution::default());
        original
            .work
            .schedule(WorkspaceWorkStep::None, rfc3339_at(now));
        original
            .transition(
                WorkspaceStatus::Failed,
                rfc3339_at(now),
                Some(TypedFailure {
                    class: "workspace_prepare_outcome_unknown".into(),
                    detail: "original prepare failed".into(),
                }),
            )
            .unwrap();
        harness
            .store
            .save_workspace(revision, original.clone())
            .await
            .unwrap();
        let probe = Arc::new(RecoveryProbe {
            backend: harness.backend.clone(),
            requests: Mutex::default(),
            rejection: Mutex::default(),
            lose_response: AtomicBool::new(false),
        });
        Self {
            harness,
            service,
            original,
            now,
            probe,
            homes,
        }
    }
    fn worker(&self) -> Arc<WorkspaceWorker> {
        WorkspaceWorker::new(
            WorkspaceWorkerDependencies {
                store: self.harness.store.clone(),
                adapter: Arc::new(
                    CodexAdapter::new(self.probe.clone(), CodexSandboxSpec::default())
                        .with_home_volumes(self.homes.clone()),
                ),
            },
            WorkspaceWorkerConfig::new("recovery-worker", "test"),
        )
    }
    async fn admit(&self, key: &str) {
        self.service
            .recover_prepare(
                &caller("owner", [Capability::UseOwnSandboxes]),
                &self.original.id,
                key,
            )
            .await
            .unwrap();
    }
    async fn stored(&self) -> Workspace {
        self.harness
            .store
            .get_workspace(&self.original.id)
            .await
            .unwrap()
            .unwrap()
    }
}

#[tokio::test]
async fn prepare_recovery_lost_response_replays_original_operation_then_completes_real_setup() {
    let f = Fixture::new().await;
    f.admit("first").await;
    f.probe.lose_response.store(true, Ordering::SeqCst);
    f.worker().run_one(&f.original.id, f.now).await.unwrap();
    let pending = f.stored().await;
    assert_eq!(pending.status, WorkspaceStatus::Preparing);
    assert_eq!(pending.work.step, WorkspaceWorkStep::RecoverPrepare);
    assert!(pending.ready_at.is_none());
    assert!(pending.work.prepare_recovery.as_ref().unwrap().attempts[0]
        .completion
        .is_none());
    let after_retry = f.now + Duration::from_secs(31);
    let restarted = f.worker();
    restarted
        .run_one(&f.original.id, after_retry)
        .await
        .unwrap();
    let prepared = f.stored().await;
    assert_eq!(prepared.work.step, WorkspaceWorkStep::Setup);
    assert!(prepared.ready_at.is_none());
    assert!(prepared.work.prepare_recovery.as_ref().unwrap().attempts[0]
        .completion
        .is_some());
    restarted
        .run_one(&f.original.id, after_retry)
        .await
        .unwrap();
    let ready = f.stored().await;
    assert_eq!(ready.status, WorkspaceStatus::Ready);
    assert_eq!(ready.id, f.original.id);
    assert_eq!(ready.path, f.original.path);
    assert_eq!(ready.profile, f.original.profile);
    assert_eq!(ready.work.execution, f.original.work.execution);
    assert!(ready.resolved_commit.is_some());
    assert!(ready.ready_at.is_some());
    let requests = f.probe.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0], requests[1]);
}

#[tokio::test]
async fn prepare_recovery_terminal_failure_preserves_prior_attempt_and_allows_explicit_new_key() {
    let f = Fixture::new().await;
    f.admit("first").await;
    *f.probe.rejection.lock().unwrap() = Some(AdapterError::Unsupported("old runtime".into()));
    f.worker().run_one(&f.original.id, f.now).await.unwrap();
    let failed = f.stored().await;
    assert_eq!(failed.status, WorkspaceStatus::Failed);
    assert!(failed.work.prepare_recovery.as_ref().unwrap().attempts[0]
        .failed
        .is_some());
    assert!(failed.ready_at.is_none());
    f.admit("first").await;
    assert_eq!(f.stored().await, failed);
    f.admit("second").await;
    let worker = f.worker();
    worker.run_one(&f.original.id, f.now).await.unwrap();
    worker.run_one(&f.original.id, f.now).await.unwrap();
    let ready = f.stored().await;
    assert_eq!(ready.status, WorkspaceStatus::Ready);
    let attempts = &ready.work.prepare_recovery.as_ref().unwrap().attempts;
    assert_eq!(attempts.len(), 2);
    assert!(attempts[0].failed.is_some());
    assert!(attempts[1].completion.is_some());
    let requests = f.probe.requests.lock().unwrap();
    assert_eq!(
        requests[1].previous_recovery_operation_id.as_deref(),
        Some("first")
    );
}
