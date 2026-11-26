use super::*;
use layer3_adapter::hosted::{HostedAgentOpenSettings, HostedBackend};
use layer3_adapter::provider::ProviderSandboxRequest;
use layer3_adapter::wire::{AgentOpenReceipt, FencedReceipt, FencedRequest, ResponseFrame};
use layer3_adapter::{
    AgentLaunch, ConversationChannel, ConversationUpdate, HomeVolumeAttachment, HomeVolumeReceipt,
    HomeVolumeRequest, HomeVolumeStore, RuntimeObservation,
};
use tokio::sync::{mpsc, Semaphore};

pub(super) struct Control {
    pub started: AtomicUsize,
    pub settled: AtomicUsize,
    pub active: AtomicUsize,
    pub peak: AtomicUsize,
    pub cancelled: AtomicUsize,
    pub cleanup: AtomicUsize,
    pub homes: AtomicUsize,
    pub refuse_binding_inventory: std::sync::atomic::AtomicBool,
    pub inventory_calls: AtomicUsize,
    pub gates: Vec<Semaphore>,
    pub count: usize,
    pub fail: Option<usize>,
    /// The injected failure: a transient stream/assignment timeout, or a refusal.
    pub transient: Option<&'static str>,
    /// Set once startup has returned and the background sweep takes over.
    pub sweeping: std::sync::atomic::AtomicBool,
    pub delay: Duration,
}

impl Control {
    fn new(
        count: usize,
        fail: Option<usize>,
        transient: Option<&'static str>,
        delay: Duration,
    ) -> Arc<Self> {
        Arc::new(Self {
            started: AtomicUsize::new(0),
            settled: AtomicUsize::new(0),
            active: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
            cancelled: AtomicUsize::new(0),
            cleanup: AtomicUsize::new(0),
            homes: AtomicUsize::new(0),
            refuse_binding_inventory: std::sync::atomic::AtomicBool::new(false),
            inventory_calls: AtomicUsize::new(0),
            gates: (0..count).map(|_| Semaphore::new(0)).collect(),
            count,
            fail,
            transient,
            sweeping: std::sync::atomic::AtomicBool::new(false),
            delay,
        })
    }
    fn cleanup_barrier(&self) {
        assert_eq!(self.active.load(Ordering::SeqCst), 0);
        if self.sweeping.load(Ordering::SeqCst) {
            // The sweep revisits bindings; only startup cleanup is barrier-bound.
            return;
        }
        assert_eq!(self.settled.load(Ordering::SeqCst), self.count);
        assert!(self.fail.is_none());
    }
}

struct Attempt<'a> {
    control: &'a Control,
    settled: bool,
}
impl Drop for Attempt<'_> {
    fn drop(&mut self) {
        self.control.active.fetch_sub(1, Ordering::SeqCst);
        if !self.settled {
            self.control.cancelled.fetch_add(1, Ordering::SeqCst);
        }
    }
}

struct Backend {
    inner: InMemoryHostedBackend,
    control: Arc<Control>,
    headroom: Option<usize>,
}

#[async_trait::async_trait]
impl HostedBackend for Backend {
    fn startup_binding_headroom(&self, _: &[RuntimeHandle]) -> Option<usize> {
        self.headroom
    }
    async fn provision(
        &self,
        request: ProviderSandboxRequest,
    ) -> layer3_adapter::Result<RuntimeHandle> {
        self.inner.provision(request).await
    }
    async fn inventory(&self) -> layer3_adapter::Result<Vec<RuntimeObservation>> {
        let call = self.control.inventory_calls.fetch_add(1, Ordering::SeqCst);
        if !self.control.refuse_binding_inventory.load(Ordering::SeqCst) {
            return self.inner.inventory().await;
        }
        if call == 0 {
            // Omit the binding from startup's initial snapshot so it refreshes.
            return Ok(Vec::new());
        }
        Err(layer3_adapter::AdapterError::Rejected(
            "native home identity changed or disappeared".into(),
        ))
    }
    async fn adopt(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
    ) -> layer3_adapter::Result<Option<RuntimeObservation>> {
        let index = self.control.started.fetch_add(1, Ordering::SeqCst);
        let active = self.control.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.control.peak.fetch_max(active, Ordering::SeqCst);
        let mut attempt = Attempt {
            control: &self.control,
            settled: false,
        };
        if self.control.delay.is_zero() {
            self.control.gates[index].acquire().await.unwrap().forget();
        } else {
            tokio::time::sleep(self.control.delay).await;
        }
        let result = if self.control.fail == Some(index) {
            Err(match self.control.transient {
                Some(detail) => layer3_adapter::AdapterError::Unavailable(detail.into()),
                None => layer3_adapter::AdapterError::Rejected(
                    "injected startup adoption refusal".into(),
                ),
            })
        } else {
            self.inner.adopt(handle, request).await
        };
        self.control.settled.fetch_add(1, Ordering::SeqCst);
        attempt.settled = true;
        result
    }
    async fn reclaim(&self, row: &RuntimeObservation) -> layer3_adapter::Result<()> {
        self.control.cleanup_barrier();
        self.control.cleanup.fetch_add(1, Ordering::SeqCst);
        self.inner.reclaim(row).await
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

struct Homes(Arc<Control>);
#[async_trait::async_trait]
impl HomeVolumeStore for Homes {
    async fn inventory(&self) -> layer3_adapter::Result<Vec<HomeVolumeAttachment>> {
        self.0.cleanup_barrier();
        self.0.homes.fetch_add(1, Ordering::SeqCst);
        Ok(Vec::new())
    }
    async fn prepare(&self, _: &HomeVolumeRequest) -> layer3_adapter::Result<HomeVolumeReceipt> {
        panic!("startup cannot create a home")
    }
    async fn verify(&self, _: &HomeVolumeAttachment) -> layer3_adapter::Result<()> {
        panic!("fixture has no bound homes")
    }
    async fn remove(
        &self,
        _: &HomeVolumeRequest,
        _: Option<&HomeVolumeReceipt>,
    ) -> layer3_adapter::Result<()> {
        panic!("fixture has no orphan homes")
    }
}

pub(super) struct Fixture {
    pub reconciler: SandboxReconciler,
    pub control: Arc<Control>,
    pub authority: BTreeMap<SandboxId, Sandbox>,
    pub inventory: Vec<RuntimeObservation>,
    pub now: SystemTime,
}

pub(super) async fn fixture(
    count: usize,
    headroom: Option<usize>,
    fail: Option<usize>,
    delay: Duration,
) -> Fixture {
    fixture_with(count, headroom, fail, None, delay).await
}

pub(super) async fn fixture_with(
    count: usize,
    headroom: Option<usize>,
    fail: Option<usize>,
    transient: Option<&'static str>,
    delay: Duration,
) -> Fixture {
    let harness = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    let (base, now) = active_with_exact_image(&harness).await;
    assert!(base.lifecycle.home.is_none());
    let mut authority = BTreeMap::from([(base.id.clone(), base.clone())]);
    for index in 1..count {
        let mut sandbox = base.clone();
        sandbox.id = SandboxId(format!("sbx_startup_{index:03}"));
        sandbox.name = format!("startup-{index}");
        sandbox.revision = 1;
        let handle = harness
            .adapter
            .provision_runtime(&layer3_adapter::RuntimeProvisionRequest::new(
                1,
                sandbox_spec(&sandbox, "test").unwrap(),
            ))
            .await
            .unwrap();
        let binding = sandbox.runtime.as_mut().unwrap();
        binding.runtime_id = handle.0;
        binding.boot_id = None;
        sandbox.validate().unwrap();
        harness.store.create_sandbox(sandbox.clone()).await.unwrap();
        authority.insert(sandbox.id.clone(), sandbox);
    }
    let mut orphan = sandbox_spec(&base, "test").unwrap();
    orphan.sandbox_id = "sbx_startup_orphan".into();
    harness.adapter.provision(&orphan).await.unwrap();
    let inventory = harness.adapter.runtime_inventory().await.unwrap();
    let control = Control::new(count, fail, transient, delay);
    let adapter = CodexAdapter::with_backend(
        Backend {
            inner: harness.backend.clone(),
            control: control.clone(),
            headroom,
        },
        CodexSandboxSpec::default(),
    )
    .with_home_volumes(Arc::new(Homes(control.clone())));
    Fixture {
        reconciler: reconciler(&harness, Arc::new(adapter)),
        control,
        authority,
        inventory,
        now,
    }
}
