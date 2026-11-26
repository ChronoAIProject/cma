use super::*;

use layer3_adapter::hosted::{HostedAgentOpenSettings, HostedBackend};
use layer3_adapter::provider::ProviderSandboxRequest;
use layer3_adapter::wire::{AgentOpenReceipt, FencedReceipt, FencedRequest, ResponseFrame};
use layer3_adapter::{AgentLaunch, ConversationChannel, ConversationUpdate, RuntimeObservation};
use tokio::sync::mpsc;

#[tokio::test]
async fn stale_inventory_is_refetched_before_declaring_a_bound_runtime_missing() {
    let harness = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    let (active, now) = active_with_exact_image(&harness).await;
    let backend = InventoryProbe::new(harness.backend.clone(), vec![Ok(Vec::new())]);
    let calls = backend.calls.clone();
    let adapter: Arc<dyn SandboxAdapter> = Arc::new(CodexAdapter::with_backend(
        backend,
        CodexSandboxSpec::default(),
    ));
    reconciler(&harness, adapter.clone())
        .initialize(now)
        .await
        .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(harness.stored(&active.id).await, active);
    adapter
        .resources(&RuntimeHandle(active.runtime.unwrap().runtime_id))
        .await
        .unwrap();
}

#[tokio::test]
async fn incomplete_provider_inventory_cannot_recover_or_reclaim_anything() {
    let harness = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    let (active, now) = active_with_exact_image(&harness).await;
    let backend = InventoryProbe::new(
        harness.backend.clone(),
        vec![Err(layer3_adapter::AdapterError::Unavailable(
            "inventory page unavailable".into(),
        ))],
    );
    let adapter = Arc::new(CodexAdapter::with_backend(
        backend,
        CodexSandboxSpec::default(),
    ));
    assert!(reconciler(&harness, adapter).initialize(now).await.is_err());
    assert_eq!(harness.stored(&active.id).await, active);
    assert_eq!(harness.adapter.runtime_inventory().await.unwrap().len(), 1);
}

#[tokio::test]
async fn incomplete_database_authority_cannot_reclaim_provider_only_runtimes() {
    let harness = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    let created = harness.create().await;
    let mut spec = sandbox_spec(&created, "test").unwrap();
    spec.sandbox_id = "sbx_orphan".into();
    let orphan = harness.adapter.provision(&spec).await.unwrap();
    let mut corrupt = created.clone();
    corrupt.revision += 1;
    corrupt.work.next_fence = 0;
    harness
        .store
        .save_sandbox(created.revision, corrupt.clone())
        .await
        .unwrap();
    assert!(reconciler(&harness, fresh_adapter(&harness, None))
        .initialize(start_time(&created))
        .await
        .is_err());
    assert!(harness.backend.snapshot(&orphan).await.is_ok());
    assert_eq!(harness.stored(&created.id).await, corrupt);
}

#[tokio::test]
async fn bound_runtime_with_conflicting_sandbox_identity_is_not_adopted_or_reclaimed() {
    let harness = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    let (mut active, now) = active_with_exact_image(&harness).await;
    let mut spec = sandbox_spec(&active, "test").unwrap();
    spec.sandbox_id = "sbx_other".into();
    let other = harness.adapter.provision(&spec).await.unwrap();
    let revision = active.revision;
    active.runtime.as_mut().unwrap().runtime_id = other.0.clone();
    active.revision += 1;
    harness
        .store
        .save_sandbox(revision, active.clone())
        .await
        .unwrap();
    assert!(reconciler(&harness, fresh_adapter(&harness, None))
        .initialize(now)
        .await
        .is_err());
    assert_eq!(harness.stored(&active.id).await, active);
    assert!(harness.backend.snapshot(&other).await.is_ok());
}

#[tokio::test]
async fn periodic_reconciliation_exits_on_shutdown() {
    let harness = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    let (shutdown, signal) = tokio::sync::watch::channel(false);
    let activity = crate::worker_supervision::Workers::new().get("sandbox_reconciliation");
    let task =
        reconciler(&harness, fresh_adapter(&harness, None)).spawn_supervised(signal, activity);
    shutdown.send(true).unwrap();
    task.await.unwrap();
}

pub(in crate::sandbox::worker_tests) struct InventoryProbe {
    inner: InMemoryHostedBackend,
    replies: Mutex<VecDeque<layer3_adapter::Result<Vec<RuntimeObservation>>>>,
    calls: Arc<AtomicUsize>,
}

impl InventoryProbe {
    pub(in crate::sandbox::worker_tests) fn new(
        inner: InMemoryHostedBackend,
        replies: Vec<layer3_adapter::Result<Vec<RuntimeObservation>>>,
    ) -> Self {
        Self {
            inner,
            replies: Mutex::new(replies.into()),
            calls: Arc::new(AtomicUsize::new(0)),
        }
    }
}

#[async_trait::async_trait]
impl HostedBackend for InventoryProbe {
    async fn provision(
        &self,
        request: ProviderSandboxRequest,
    ) -> layer3_adapter::Result<RuntimeHandle> {
        self.inner.provision(request).await
    }

    async fn inventory(&self) -> layer3_adapter::Result<Vec<RuntimeObservation>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let reply = self.replies.lock().unwrap().pop_front();
        match reply {
            Some(reply) => reply,
            None => self.inner.inventory().await,
        }
    }

    async fn adopt(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
    ) -> layer3_adapter::Result<Option<RuntimeObservation>> {
        self.inner.adopt(handle, request).await
    }

    async fn reclaim(&self, observation: &RuntimeObservation) -> layer3_adapter::Result<()> {
        self.inner.reclaim(observation).await
    }

    async fn exchange(
        &self,
        handle: &RuntimeHandle,
        request: RequestFrame,
    ) -> layer3_adapter::Result<ResponseFrame> {
        self.inner.exchange(handle, request).await
    }

    async fn open_agent(
        &self,
        handle: &RuntimeHandle,
        launch: FencedRequest<AgentLaunch>,
        resume: Option<String>,
        settings: HostedAgentOpenSettings,
        updates: mpsc::Sender<ConversationUpdate>,
    ) -> layer3_adapter::Result<(ConversationChannel, FencedReceipt<AgentOpenReceipt>)> {
        self.inner
            .open_agent(handle, launch, resume, settings, updates)
            .await
    }

    async fn teardown(&self, handle: &RuntimeHandle) -> layer3_adapter::Result<()> {
        self.inner.teardown(handle).await
    }
}
