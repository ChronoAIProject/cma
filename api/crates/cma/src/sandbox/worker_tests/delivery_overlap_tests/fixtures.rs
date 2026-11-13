use super::*;
use layer3_adapter::hosted::{HostedAgentOpenSettings, HostedBackend};
use layer3_adapter::provider::ProviderSandboxRequest;
use layer3_adapter::wire::{AgentOpenReceipt, FencedReceipt, FencedRequest, ResponseFrame};
use layer3_adapter::{
    AdapterError, AgentLaunch, ConversationChannel, ConversationUpdate, RuntimeObservation,
};

pub(super) struct Credentials {
    inner: Arc<MemoryCredentials>,
    pub delay: Duration,
    pub started: AtomicUsize,
    pub active: AtomicUsize,
    pub failure: Mutex<Option<CredentialError>>,
}

struct Active<'a>(&'a AtomicUsize);
impl Drop for Active<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

#[async_trait::async_trait]
impl SandboxCredentialController for Credentials {
    async fn issue(
        &self,
        sandbox: &Sandbox,
        generation: u64,
        now: SystemTime,
    ) -> Result<CredentialMaterial, CredentialError> {
        self.inner.issue(sandbox, generation, now).await
    }
    async fn material(
        &self,
        id: &SandboxId,
        generation: u64,
    ) -> Result<Option<CredentialMaterial>, CredentialError> {
        self.started.fetch_add(1, Ordering::SeqCst);
        self.active.fetch_add(1, Ordering::SeqCst);
        let _active = Active(&self.active);
        tokio::time::sleep(self.delay).await;
        if let Some(error) = self.failure.lock().unwrap().clone() {
            return Err(error);
        }
        self.inner.material(id, generation).await
    }
    async fn custodied(&self, id: &SandboxId) -> Result<Vec<CredentialMaterial>, CredentialError> {
        self.inner.custodied(id).await
    }
    async fn forget(&self, id: &SandboxId, generation: u64) -> Result<(), CredentialError> {
        self.inner.forget(id, generation).await
    }
    async fn revoke(&self, sandbox: &Sandbox, key: &str) -> Result<(), CredentialError> {
        self.inner.revoke(sandbox, key).await
    }
}

pub(super) struct Backend {
    inner: InMemoryHostedBackend,
    delay: Duration,
    pub inspections: AtomicUsize,
    pub active: AtomicUsize,
    pub deliveries: AtomicUsize,
    pub failure: Mutex<Option<AdapterError>>,
}

#[async_trait::async_trait]
impl HostedBackend for Backend {
    async fn provision(
        &self,
        request: ProviderSandboxRequest,
    ) -> layer3_adapter::Result<RuntimeHandle> {
        self.inner.provision(request).await
    }
    async fn adopt(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
    ) -> layer3_adapter::Result<Option<RuntimeObservation>> {
        self.inspections.fetch_add(1, Ordering::SeqCst);
        self.active.fetch_add(1, Ordering::SeqCst);
        let _active = Active(&self.active);
        tokio::time::sleep(self.delay).await;
        if let Some(error) = self.failure.lock().unwrap().clone() {
            return Err(error);
        }
        self.inner.adopt(handle, request).await
    }
    async fn exchange(
        &self,
        handle: &RuntimeHandle,
        request: RequestFrame,
    ) -> layer3_adapter::Result<ResponseFrame> {
        if matches!(request, RequestFrame::Credential(_)) {
            self.deliveries.fetch_add(1, Ordering::SeqCst);
        }
        self.inner.exchange(handle, request).await
    }
    async fn open_agent(
        &self,
        handle: &RuntimeHandle,
        launch: FencedRequest<AgentLaunch>,
        resume: Option<String>,
        settings: HostedAgentOpenSettings,
        updates: tokio::sync::mpsc::Sender<ConversationUpdate>,
    ) -> layer3_adapter::Result<(ConversationChannel, FencedReceipt<AgentOpenReceipt>)> {
        self.inner
            .open_agent(handle, launch, resume, settings, updates)
            .await
    }
    async fn teardown(&self, handle: &RuntimeHandle) -> layer3_adapter::Result<()> {
        self.inner.teardown(handle).await
    }
}

pub(super) struct Fixture {
    pub harness: Harness,
    pub id: SandboxId,
    pub now: SystemTime,
    pub backend: Arc<Backend>,
    pub credentials: Arc<Credentials>,
    pub worker: Arc<SandboxWorker>,
}
impl Fixture {
    pub async fn new(inspection_delay: Duration, material_delay: Duration) -> Self {
        let harness = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
        let sandbox = harness.create().await;
        let now = start_time(&sandbox);
        for _ in 0..3 {
            harness.run(&sandbox.id, now).await;
        }
        assert_eq!(
            harness.stored(&sandbox.id).await.work.step,
            SandboxWorkStep::DeliverInitialCredential
        );
        let backend = Arc::new(Backend {
            inner: harness.backend.clone(),
            delay: inspection_delay,
            inspections: AtomicUsize::new(0),
            active: AtomicUsize::new(0),
            deliveries: AtomicUsize::new(0),
            failure: Mutex::new(None),
        });
        let credentials = Arc::new(Credentials {
            inner: harness.credentials.clone(),
            delay: material_delay,
            started: AtomicUsize::new(0),
            active: AtomicUsize::new(0),
            failure: Mutex::new(None),
        });
        let adapter = Arc::new(CodexAdapter::new(
            backend.clone(),
            CodexSandboxSpec::default(),
        ));
        let worker = SandboxWorker::new(
            SandboxWorkerDependencies {
                store: harness.store.clone(),
                adapter,
                credentials: credentials.clone(),
                workspaces: Arc::new(WorkspaceRecorder {
                    actions: harness.actions.clone(),
                }),
                agents: Arc::new(AgentStopRecorder {
                    actions: harness.actions.clone(),
                }),
                registry: harness.registry.clone(),
                updates: DurableAgentUpdateRecorder::new(harness.store.clone()),
                operations: AgentOperationService::new(
                    harness.store.clone(),
                    harness.registry.clone(),
                ),
            },
            SandboxWorkerConfig::new(
                "delivery-probe",
                "test",
                runtime_image(),
                harness.policy.clone(),
            ),
        );
        Self {
            harness,
            id: sandbox.id,
            now,
            backend,
            credentials,
            worker,
        }
    }
}
