use super::*;
use crate::sandbox::workspace_worker::{
    WorkspaceWorker, WorkspaceWorkerConfig, WorkspaceWorkerDependencies,
};
use layer2_core::sandbox::store::{WorkspaceLifecycleStore, WorkspaceStore};
use layer2_core::sandbox::{
    CreateInlineWorkspace, InlineWorkspaceSource, ProvisioningExecution, TypedFailure, Workspace,
    WorkspaceService, WorkspaceStatus, WorkspaceWorkStep,
};
use layer3_adapter::hosted::{
    HostedAgentOpenSettings, HostedBackend, HostedControlTransport, ProviderHostedControlTransport,
};
use layer3_adapter::provider::ProviderSandboxRequest;
use layer3_adapter::wire::{
    AgentOpenReceipt, FencedReceipt, FencedRequest, ResponseFrame, WireFailure, WireOperation,
};
use layer3_adapter::{
    AdapterError, AgentLaunch, ConversationChannel, ConversationUpdate, RuntimeObservation,
    WorkspacePrepareRecoveryReceipt, WorkspacePrepareRecoveryRequest,
};

struct RecoveryProbe {
    transport: Option<ProviderHostedControlTransport>,
    backend: InMemoryHostedBackend,
    requests: Mutex<Vec<WorkspacePrepareRecoveryRequest>>,
    rejection: Mutex<Option<AdapterError>>,
    failure_reply: Mutex<Option<WireFailure>>,
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
        if let Some(transport) = self.transport.as_ref().filter(|_| {
            matches!(
                frame,
                RequestFrame::PrepareWorkspace(_) | RequestFrame::RecoverWorkspacePrepare(_)
            )
        }) {
            return transport.exchange(handle, frame).await;
        }
        {
            let mut reply = self.failure_reply.lock().unwrap();
            if reply
                .as_ref()
                .is_some_and(|reply| reply.operation == frame.operation())
            {
                return Ok(ResponseFrame::Failure(reply.take().unwrap()));
            }
        }
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
            transport: None,
            backend: harness.backend.clone(),
            requests: Mutex::default(),
            rejection: Mutex::default(),
            failure_reply: Mutex::default(),
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
        self.worker_with_config(WorkspaceWorkerConfig::new("recovery-worker", "test"))
    }
    fn worker_with_config(&self, config: WorkspaceWorkerConfig) -> Arc<WorkspaceWorker> {
        self.worker_with_store(self.harness.store.clone(), config)
    }
    fn worker_with_store(
        &self,
        store: Arc<dyn WorkspaceLifecycleStore>,
        config: WorkspaceWorkerConfig,
    ) -> Arc<WorkspaceWorker> {
        WorkspaceWorker::new(
            WorkspaceWorkerDependencies {
                store,
                adapter: Arc::new(
                    CodexAdapter::new(self.probe.clone(), CodexSandboxSpec::default())
                        .with_home_volumes(self.homes.clone()),
                ),
            },
            config,
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
    let config = WorkspaceWorkerConfig::new("recovery-worker", "test").with_replicas(true);
    f.worker_with_config(config)
        .run_one(&f.original.id, f.now)
        .await
        .unwrap();
    let pending = f.stored().await;
    assert_eq!(pending.status, WorkspaceStatus::Preparing);
    assert_eq!(pending.work.step, WorkspaceWorkStep::RecoverPrepare);
    assert!(pending.ready_at.is_none());
    assert!(pending.work.prepare_recovery.as_ref().unwrap().attempts[0]
        .observations
        .is_none());
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

#[tokio::test]
async fn workspace_prepare_observations_persist_first_and_terminal_across_workers() {
    use layer3_adapter::workspace_prepare_observation::{Code, Stage};

    let f = Fixture::new().await;
    let workspace = f
        .service
        .create_inline(
            &caller("owner", [Capability::UseOwnSandboxes]),
            &f.original.sandbox_id,
            CreateInlineWorkspace {
                source: InlineWorkspaceSource::Empty,
                name: Some("observed".into()),
            },
        )
        .await
        .unwrap();
    f.admit("first").await;
    for (id, operation, stage) in [
        (
            &workspace.id,
            WireOperation::PrepareWorkspace,
            Stage::PrepareRpc,
        ),
        (
            &f.original.id,
            WireOperation::RecoverWorkspacePrepare,
            Stage::RecoveryRpc,
        ),
    ] {
        for (index, (holder, wire_code)) in [
            ("first-worker", "repository_source_unavailable"),
            ("successor-worker", "workspace_prepare_outcome_unknown"),
        ]
        .into_iter()
        .enumerate()
        {
            *f.probe.failure_reply.lock().unwrap() = Some(WireFailure {
                operation,
                code: wire_code.into(),
                detail: "opaque provider diagnostic".into(),
                retryable: index == 0,
            });
            let worker = f.worker_with_config(
                WorkspaceWorkerConfig::new(holder, "test")
                    .with_replicas(true)
                    .with_prepare_observations(true),
            );
            assert!(worker
                .run_one(id, f.now + Duration::from_secs(31 * index as u64))
                .await
                .unwrap());
        }
        let stored = f.harness.store.get_workspace(id).await.unwrap().unwrap();
        assert_eq!(
            stored.failed.as_ref().unwrap().class,
            "workspace_runtime_rejected"
        );
        let (observations, expected_id, expected_digest) =
            if let Some(recovery) = &stored.work.prepare_recovery {
                let request = recovery.current_request().unwrap();
                (
                    &recovery.attempts[0].observations,
                    request.operation_id,
                    request.request_sha256,
                )
            } else {
                let request = stored.original_prepare_request().unwrap();
                (
                    &stored.work.prepare_observations,
                    request.operation_id,
                    request.request_sha256,
                )
            };
        let observations = observations.as_ref().unwrap();
        assert_eq!(observations.operation_id, expected_id);
        assert_eq!(observations.request_sha256, expected_digest);
        let terminal = observations.terminal.as_ref().unwrap();
        for (sample, code, attempt) in [
            (&observations.first, Code::RepositorySourceUnavailable, 0),
            (terminal, Code::WorkspacePrepareOutcomeUnknown, 1),
        ] {
            assert_eq!(sample.observation.stage, stage);
            assert_eq!(sample.observation.code, code);
            assert_eq!(sample.observation.wire_retryable, Some(attempt == 0));
            assert_eq!(sample.attempt, attempt);
            assert!(sample
                .writer_sha256
                .as_ref()
                .is_some_and(|digest| digest.len() == 64));
        }
        assert!(!serde_json::to_string(observations)
            .unwrap()
            .contains("opaque provider diagnostic"));
    }
}

#[path = "workspace_prepare_observations_mongo_tests.rs"]
mod observation_mongo;
