use super::*;
use crate::sandbox::workspace_worker::{
    DurableProfileWorkspaceProvisioner, WorkspaceWorker, WorkspaceWorkerConfig,
    WorkspaceWorkerDependencies,
};
use layer2_core::sandbox::store::{WorkspaceStore as _, WorkspaceWorkerStore as _};
use layer2_core::sandbox::{WorkspaceService, WorkspaceStatus, WorkspaceWorkStep};
use layer3_adapter::hosted::{HostedAgentOpenSettings, HostedBackend};
use layer3_adapter::provider::ProviderSandboxRequest;
use layer3_adapter::wire::{AgentOpenReceipt, FencedReceipt, FencedRequest, ResponseFrame};
use layer3_adapter::{
    AdapterError, AgentLaunch, ConversationChannel, ConversationUpdate, RuntimeContractMismatch,
    RuntimeObservation,
};

#[derive(Clone, Copy, Debug)]
enum AuthorityChange {
    Credential,
    Status,
    RuntimeMissing,
    Generation,
    WorkspaceLease,
}

struct AdoptionProbe {
    backend: InMemoryHostedBackend,
    store: Arc<InMemorySandboxStore>,
    sandbox_id: SandboxId,
    events: ActionLog,
    rejection: Mutex<Option<AdapterError>>,
    after_prepare: Mutex<Option<AuthorityChange>>,
}

impl AdoptionProbe {
    async fn change_authority(&self, change: AuthorityChange, handle: &RuntimeHandle) {
        if matches!(change, AuthorityChange::RuntimeMissing) {
            self.backend.teardown(handle).await.unwrap();
            return;
        }
        if matches!(change, AuthorityChange::WorkspaceLease) {
            let workspace = self
                .store
                .list_workspaces(&self.sandbox_id, &Default::default())
                .await
                .unwrap()
                .items
                .pop()
                .unwrap();
            let expiry = workspace.work.lease.unwrap().expires_at;
            self.store
                .claim_workspace_work(
                    &workspace.id,
                    "successor",
                    expiry.clone(),
                    rfc3339_at(stamp_time(&expiry) + Duration::from_secs(60)),
                )
                .await
                .unwrap()
                .unwrap();
            return;
        }
        let mut parent = self
            .store
            .get_sandbox(&self.sandbox_id)
            .await
            .unwrap()
            .unwrap();
        let revision = parent.revision;
        match change {
            AuthorityChange::Credential => parent.credential.delivered = false,
            AuthorityChange::Status => parent.status = SandboxStatus::Retiring,
            AuthorityChange::Generation => {
                let binding = parent.runtime.as_mut().unwrap();
                let generation = binding.provision_generation.unwrap() + 1;
                binding.provision_generation = Some(generation);
                binding.runtime_id = format!("replacement-{}", binding.runtime_id);
                parent.lifecycle.runtime_generation_high_water = generation;
            }
            _ => unreachable!(),
        }
        parent.revision += 1;
        self.store.save_sandbox(revision, parent).await.unwrap();
    }
}

#[async_trait::async_trait]
impl HostedBackend for AdoptionProbe {
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
        self.events.push("adopt");
        if let Some(error) = self.rejection.lock().unwrap().clone() {
            return Err(error);
        }
        self.backend.adopt(handle, request).await
    }

    async fn exchange(
        &self,
        handle: &RuntimeHandle,
        request: RequestFrame,
    ) -> layer3_adapter::Result<ResponseFrame> {
        let prepare = matches!(request, RequestFrame::PrepareWorkspace(_));
        match request {
            RequestFrame::PrepareWorkspace(_) => self.events.push("prepare"),
            RequestFrame::WorkspaceSetup(_) => self.events.push("setup"),
            _ => (),
        }
        let result = self.backend.exchange(handle, request).await;
        if prepare && result.is_ok() {
            let change = self.after_prepare.lock().unwrap().take();
            if let Some(change) = change {
                self.change_authority(change, handle).await;
            }
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

struct Fixture {
    harness: Harness,
    parent: Sandbox,
    now: SystemTime,
    probe: Arc<AdoptionProbe>,
    worker: Arc<SandboxWorker>,
    provisioner: Arc<DurableProfileWorkspaceProvisioner>,
    reference: WorkspaceProfileRef,
}

impl Fixture {
    async fn new() -> Self {
        let harness = Harness::new(Vec::new(), 1, Duration::from_secs(60)).await;
        let created = harness.create().await;
        let now = start_time(&created);
        for _ in 0..4 {
            harness.run(&created.id, now).await;
        }
        let parent = harness.stored(&created.id).await;
        assert_eq!(parent.work.step, SandboxWorkStep::CreateProfileWorkspace(0));
        assert!(parent.credential.delivered);
        let probe = Arc::new(AdoptionProbe {
            backend: harness.backend.clone(),
            store: harness.store.clone(),
            sandbox_id: parent.id.clone(),
            events: ActionLog::default(),
            rejection: Mutex::new(None),
            after_prepare: Mutex::new(None),
        });
        let adapter = Arc::new(CodexAdapter::new(
            probe.clone(),
            CodexSandboxSpec::default(),
        ));
        let profiles = Arc::new(InMemoryProfileStore::default());
        let reference = seed_workspace_profile(&profiles, 0).await;
        let provisioner = Arc::new(DurableProfileWorkspaceProvisioner::new(
            WorkspaceService::new(harness.store.clone(), profiles),
            WorkspaceWorker::new(
                WorkspaceWorkerDependencies {
                    store: harness.store.clone(),
                    adapter: adapter.clone(),
                },
                WorkspaceWorkerConfig::new("workspace", "test"),
            ),
        ));
        let dependencies = SandboxWorkerDependencies {
            store: harness.store.clone(),
            adapter,
            credentials: harness.credentials.clone(),
            workspaces: provisioner.clone(),
            agents: Arc::new(AgentStopRecorder {
                actions: harness.actions.clone(),
            }),
            registry: harness.registry.clone(),
            updates: DurableAgentUpdateRecorder::new(harness.store.clone()),
            operations: AgentOperationService::new(harness.store.clone(), harness.registry.clone()),
        };
        let mut config =
            SandboxWorkerConfig::new("parent", "test", runtime_image(), harness.policy.clone());
        config.lease_duration = Duration::from_secs(60);
        config.retry_delay = Duration::from_secs(5);
        Self {
            harness,
            parent,
            now,
            probe,
            worker: SandboxWorker::new(dependencies, config),
            provisioner,
            reference,
        }
    }

    async fn replay(&self) -> Result<(), SandboxDependencyError> {
        self.provisioner
            .create_profile_workspace(
                &self.parent,
                &RuntimeHandle(self.parent.runtime.as_ref().unwrap().runtime_id.clone()),
                &self.reference,
                0,
                "parent-replay",
            )
            .await
    }

    async fn child(&self) -> layer2_core::sandbox::Workspace {
        let mut children = self
            .harness
            .store
            .list_workspaces(&self.parent.id, &Default::default())
            .await
            .unwrap()
            .items;
        assert_eq!(children.len(), 1);
        children.pop().unwrap()
    }
}

#[tokio::test]
async fn durable_provisioner_keeps_each_effect_proof_and_removes_only_outer_adoption() {
    let fixture = Fixture::new().await;
    assert!(fixture
        .worker
        .run_one(&fixture.parent.id, fixture.now)
        .await
        .unwrap());
    assert_eq!(
        fixture.probe.events.snapshot(),
        ["adopt", "prepare", "adopt", "setup"]
    );
    let parent = fixture.harness.stored(&fixture.parent.id).await;
    assert_eq!(parent.work.step, SandboxWorkStep::Activate);
    let child = fixture.child().await;
    assert_eq!(child.status, WorkspaceStatus::Ready);
    let requests = fixture.harness.backend.requests().await;
    let prepare = requests
        .iter()
        .filter_map(|(_, request)| match request {
            RequestFrame::PrepareWorkspace(request) => Some(request),
            _ => None,
        })
        .collect::<Vec<_>>();
    let setup = requests
        .iter()
        .filter_map(|(_, request)| match request {
            RequestFrame::WorkspaceSetup(request) => Some(request),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(prepare.len(), 1);
    assert_eq!(setup.len(), 1);
    assert_eq!(
        prepare[0].operation_id,
        child.operation_id(WorkspaceWorkStep::Prepare)
    );
    assert_eq!(
        setup[0].operation_id,
        child.operation_id(WorkspaceWorkStep::Setup)
    );
    assert_eq!(prepare[0].workspace_id, child.id.to_string());
    assert_eq!(setup[0].workspace_id, child.id.to_string());
}

#[tokio::test]
async fn already_ready_replay_still_requires_current_runtime_authority() {
    let fixture = Fixture::new().await;
    fixture.replay().await.unwrap();
    fixture.replay().await.unwrap();
    assert_eq!(
        fixture.probe.events.snapshot(),
        ["adopt", "prepare", "adopt", "setup", "adopt"]
    );
    let handle = RuntimeHandle(fixture.parent.runtime.as_ref().unwrap().runtime_id.clone());
    fixture.probe.backend.teardown(&handle).await.unwrap();
    assert!(matches!(
        fixture.replay().await,
        Err(SandboxDependencyError::Transient(_))
    ));
    assert_eq!(
        fixture.probe.events.snapshot(),
        ["adopt", "prepare", "adopt", "setup", "adopt", "adopt"]
    );
}

#[tokio::test]
async fn alternate_provisioner_retains_outer_adoption_before_invocation() {
    for refuse in [false, true] {
        let fixture = Fixture::new().await;
        if refuse {
            *fixture.probe.rejection.lock().unwrap() =
                Some(AdapterError::Rejected("refused".into()));
        }
        let worker = fixture.harness.worker_with_adapter(
            "alternate",
            Arc::new(CodexAdapter::new(
                fixture.probe.clone(),
                CodexSandboxSpec::default(),
            )),
        );
        assert!(worker
            .run_one(&fixture.parent.id, fixture.now)
            .await
            .unwrap());
        assert_eq!(fixture.probe.events.snapshot(), ["adopt"]);
        let actions = fixture.harness.actions.snapshot();
        assert_eq!(
            actions
                .iter()
                .any(|action| action.starts_with("workspace:")),
            !refuse
        );
    }
}

#[tokio::test]
async fn invalid_current_parent_refuses_effects_even_with_a_valid_caller_snapshot() {
    for change in [
        AuthorityChange::Credential,
        AuthorityChange::Status,
        AuthorityChange::RuntimeMissing,
        AuthorityChange::Generation,
    ] {
        let fixture = Fixture::new().await;
        let handle = RuntimeHandle(fixture.parent.runtime.as_ref().unwrap().runtime_id.clone());
        fixture.probe.change_authority(change, &handle).await;
        assert!(fixture.replay().await.is_err(), "{change:?}");
        let events = fixture.probe.events.snapshot();
        assert!(
            events.iter().all(|event| event == "adopt"),
            "{change:?}: {events:?}"
        );
    }
}

#[tokio::test]
async fn already_ready_replay_checks_fresh_parent_credential_before_adoption() {
    let fixture = Fixture::new().await;
    fixture.replay().await.unwrap();
    let before = fixture.probe.events.snapshot();
    let handle = RuntimeHandle(fixture.parent.runtime.as_ref().unwrap().runtime_id.clone());
    fixture
        .probe
        .change_authority(AuthorityChange::Credential, &handle)
        .await;
    assert!(matches!(
        fixture.replay().await,
        Err(SandboxDependencyError::Transient(_))
    ));
    assert_eq!(fixture.probe.events.snapshot(), before);
}

#[tokio::test]
async fn adoption_failures_keep_parent_categories_and_prevent_workspace_effects() {
    for (error, class) in [
        (AdapterError::Unavailable("unavailable".into()), None),
        (
            AdapterError::AtCapacity("capacity".into()),
            Some("resource_quota"),
        ),
        (
            AdapterError::Rejected("refused".into()),
            Some("terminal_validation"),
        ),
        (
            AdapterError::RuntimeContractMismatch(RuntimeContractMismatch {
                image: runtime_image(),
                expected: "3".into(),
                found: Some("2".into()),
            }),
            Some("terminal_security"),
        ),
    ] {
        let fixture = Fixture::new().await;
        *fixture.probe.rejection.lock().unwrap() = Some(error);
        assert!(fixture
            .worker
            .run_one(&fixture.parent.id, fixture.now)
            .await
            .unwrap());
        assert_eq!(fixture.probe.events.snapshot(), ["adopt"]);
        let parent = fixture.harness.stored(&fixture.parent.id).await;
        match class {
            Some(class) => {
                assert_eq!(parent.status, SandboxStatus::Failed);
                assert_eq!(parent.failure.unwrap().class, class);
            }
            None => {
                assert_eq!(parent.status, SandboxStatus::Installing);
                assert_eq!(parent.work.step, SandboxWorkStep::CreateProfileWorkspace(0));
                assert_eq!(parent.work.attempts, 1);
            }
        }
    }
}

#[tokio::test]
async fn authority_changes_after_prepare_refuse_setup_and_parent_activation() {
    for change in [
        AuthorityChange::Credential,
        AuthorityChange::Status,
        AuthorityChange::RuntimeMissing,
        AuthorityChange::Generation,
        AuthorityChange::WorkspaceLease,
    ] {
        let fixture = Fixture::new().await;
        *fixture.probe.after_prepare.lock().unwrap() = Some(change);
        let result = fixture
            .worker
            .run_one(&fixture.parent.id, fixture.now)
            .await;
        assert!(
            matches!(result, Ok(true) | Err(SandboxStoreError::Conflict)),
            "{change:?}: {result:?}"
        );
        let events = fixture.probe.events.snapshot();
        assert_eq!(
            events
                .iter()
                .filter(|event| event.as_str() == "prepare")
                .count(),
            1,
            "{change:?}"
        );
        assert!(
            !events.iter().any(|event| event == "setup"),
            "{change:?}: {events:?}"
        );
        let parent = fixture.harness.stored(&fixture.parent.id).await;
        assert_ne!(parent.work.step, SandboxWorkStep::Activate, "{change:?}");
        let child = fixture.child().await;
        assert_ne!(child.status, WorkspaceStatus::Ready, "{change:?}");
        if matches!(change, AuthorityChange::WorkspaceLease) {
            assert_eq!(child.work.lease.as_ref().unwrap().holder, "successor");
            assert_eq!(child.work.step, WorkspaceWorkStep::Prepare);
        }
    }
}
