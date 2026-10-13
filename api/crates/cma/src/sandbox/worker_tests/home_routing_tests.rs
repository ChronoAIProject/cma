use super::*;
use layer3_adapter::hosted::{HostedBackend, RuntimeBackends, RuntimeRoutingStore};
use layer3_adapter::RoutedHomeVolumes;

#[derive(Default)]
struct Routes(tokio::sync::Mutex<BTreeMap<(String, u64), bool>>);
#[async_trait::async_trait]
impl RuntimeRoutingStore for Routes {
    async fn select(
        &self,
        id: &str,
        generation: u64,
        default: bool,
    ) -> layer3_adapter::Result<bool> {
        Ok(*self
            .0
            .lock()
            .await
            .entry((id.into(), generation))
            .or_insert(default))
    }
}
struct Routed {
    local: Arc<Homes>,
    remote: Arc<Homes>,
    routes: Arc<Routes>,
    native: InMemoryHostedBackend,
    native_deletes: Arc<AtomicUsize>,
}
impl Routed {
    fn new(harness: &Harness) -> Self {
        Self {
            local: Homes::for_cluster(harness, "local"),
            remote: Homes::for_cluster(harness, "remote"),
            routes: Default::default(),
            native: Default::default(),
            native_deletes: Default::default(),
        }
    }
    fn worker(&self, harness: &Harness, native_default: bool, seed: &str) -> Arc<SandboxWorker> {
        let mut policy = home_policy();
        policy.template_name = seed.into();
        policy.template_generation = format!("{seed}-uid");
        let homes = Arc::new(
            RoutedHomeVolumes::new(
                Some((self.local.clone(), home_policy())),
                Some((self.remote.clone(), policy)),
            )
            .unwrap(),
        );
        let backend = RuntimeBackends {
            opensandbox: Some(Arc::new(harness.backend.clone())),
            agent_sandbox: Some(Arc::new(NativeMemory(
                self.native.clone(),
                self.native_deletes.clone(),
            ))),
            home_routes: Some(homes.clone()),
            allocate_agent_sandbox: native_default,
            routing: self.routes.clone(),
        };
        worker_with_adapter(
            harness,
            Arc::new(
                CodexAdapter::with_backend(backend, CodexSandboxSpec::default())
                    .with_home_volumes(homes),
            ),
            "routed-worker",
        )
    }
}

#[tokio::test]
async fn cold_native_home_is_durable_before_allocation_and_keeps_cluster_seed_after_retry() {
    let harness = Harness::new(vec![], 0, Duration::from_secs(60)).await;
    let created = harness.create().await;
    let now = start_time(&created);
    let routed = Routed::new(&harness);
    routed.remote.lose_response.store(true, Ordering::SeqCst);
    routed
        .worker(&harness, true, "remote-seed")
        .run_one(&created.id, now)
        .await
        .unwrap();
    let interrupted = harness.stored(&created.id).await;
    let reserved = interrupted.lifecycle.home.as_ref().unwrap();
    assert!(reserved.receipt.is_none() && reserved.native_pool.is_none());
    assert_eq!(reserved.request.scope.as_ref(), Some(&routed.remote.scope));
    assert_eq!(reserved.request.policy.template_name, "remote-seed");
    assert_eq!(routed.remote.prepare_calls.load(Ordering::SeqCst), 1);
    assert_eq!(routed.local.prepare_calls.load(Ordering::SeqCst), 0);
    assert!(harness.actions.snapshot().is_empty());
    let reopened = routed.worker(&harness, false, "changed-seed");
    for step in 1..6 {
        reopened
            .run_one(&created.id, now + Duration::from_secs(step * 31))
            .await
            .unwrap();
    }
    let active = harness.stored(&created.id).await;
    assert_eq!(active.status, SandboxStatus::Active);
    assert_eq!(
        active.lifecycle.home.as_ref().unwrap().request,
        reserved.request
    );
    assert_eq!(routed.remote.prepare_calls.load(Ordering::SeqCst), 2);
    assert_eq!(routed.local.prepare_calls.load(Ordering::SeqCst), 0);
    harness
        .service
        .delete(
            &caller("owner", [Capability::DeleteOwnSandbox]),
            &created.id,
        )
        .await
        .unwrap();
    for step in 6..12 {
        reopened
            .run_one(&created.id, now + Duration::from_secs(step * 31))
            .await
            .unwrap();
    }
    assert_eq!(
        harness.stored(&created.id).await.status,
        SandboxStatus::Retired
    );
    assert!(routed.remote.inventory().await.unwrap().is_empty());
    assert!(routed.local.inventory().await.unwrap().is_empty());
}

#[tokio::test]
async fn historical_unattached_home_preserves_local_request_when_native_becomes_default() {
    let harness = Harness::new(vec![], 0, Duration::from_secs(60)).await;
    let created = harness.create().await;
    let now = start_time(&created);
    let routed = Routed::new(&harness);
    routed.local.lose_response.store(true, Ordering::SeqCst);
    worker(&harness, routed.local.clone(), "legacy-worker")
        .run_one(&created.id, now)
        .await
        .unwrap();
    let old = harness.stored(&created.id).await.lifecycle.home.unwrap();
    assert!(old.receipt.is_none() && old.request.scope.is_none());
    let reopened = routed.worker(&harness, true, "remote-seed");
    for step in 1..6 {
        reopened
            .run_one(&created.id, now + Duration::from_secs(step * 31))
            .await
            .unwrap();
    }
    let active = harness.stored(&created.id).await;
    assert_eq!(active.status, SandboxStatus::Active);
    assert_eq!(active.lifecycle.home.as_ref().unwrap().request, old.request);
    assert_eq!(routed.local.prepare_calls.load(Ordering::SeqCst), 2);
    assert_eq!(routed.remote.prepare_calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        routed.routes.0.lock().await.get(&(created.id.0, 1)),
        Some(&false)
    );
}

#[derive(Clone)]
struct NativeMemory(InMemoryHostedBackend, Arc<AtomicUsize>);
impl NativeMemory {
    fn original(handle: &RuntimeHandle) -> RuntimeHandle {
        RuntimeHandle::new(
            handle
                .as_str()
                .strip_prefix("agent-sandbox:")
                .expect("native handle"),
        )
    }
    fn native(handle: RuntimeHandle) -> RuntimeHandle {
        RuntimeHandle::new(format!("agent-sandbox:{}", handle.as_str()))
    }
}
#[async_trait::async_trait]
impl HostedBackend for NativeMemory {
    async fn provision(
        &self,
        request: layer3_adapter::provider::ProviderSandboxRequest,
    ) -> layer3_adapter::Result<RuntimeHandle> {
        self.0.provision(request).await.map(Self::native)
    }
    async fn inventory(&self) -> layer3_adapter::Result<Vec<layer3_adapter::RuntimeObservation>> {
        Ok(self
            .0
            .inventory()
            .await?
            .into_iter()
            .map(|mut row| {
                row.handle = Self::native(row.handle);
                row
            })
            .collect())
    }
    async fn adopt(
        &self,
        handle: &RuntimeHandle,
        request: &layer3_adapter::provider::ProviderSandboxRequest,
    ) -> layer3_adapter::Result<Option<layer3_adapter::RuntimeObservation>> {
        Ok(self
            .0
            .adopt(&Self::original(handle), request)
            .await?
            .map(|mut row| {
                row.handle = handle.clone();
                row
            }))
    }
    async fn exchange(
        &self,
        handle: &RuntimeHandle,
        request: RequestFrame,
    ) -> layer3_adapter::Result<layer3_adapter::wire::ResponseFrame> {
        self.0.exchange(&Self::original(handle), request).await
    }
    async fn teardown_retained(&self, handle: &RuntimeHandle) -> layer3_adapter::Result<()> {
        self.1.fetch_add(1, Ordering::SeqCst);
        self.0.teardown(&Self::original(handle)).await
    }
    async fn teardown(&self, handle: &RuntimeHandle) -> layer3_adapter::Result<()> {
        self.0.teardown(&Self::original(handle)).await
    }
    async fn open_agent(
        &self,
        _: &RuntimeHandle,
        _: layer3_adapter::wire::FencedRequest<layer3_adapter::AgentLaunch>,
        _: Option<String>,
        _: layer3_adapter::hosted::HostedAgentOpenSettings,
        _: tokio::sync::mpsc::Sender<layer3_adapter::ConversationUpdate>,
    ) -> layer3_adapter::Result<(
        layer3_adapter::ConversationChannel,
        layer3_adapter::wire::FencedReceipt<layer3_adapter::wire::AgentOpenReceipt>,
    )> {
        panic!("fixture creates no agents")
    }
}

#[tokio::test]
async fn retiring_historical_native_handle_cannot_delete_in_new_remote_cluster() {
    let harness = Harness::new(vec![], 0, Duration::from_secs(60)).await;
    let created = harness.create().await;
    let now = start_time(&created);
    let routed = Routed::new(&harness);
    let legacy_worker = worker(&harness, routed.local.clone(), "legacy-worker");
    for step in 0..6 {
        legacy_worker
            .run_one(&created.id, now + Duration::from_secs(step))
            .await
            .unwrap();
    }
    let mut record = harness.stored(&created.id).await;
    assert_eq!(record.status, SandboxStatus::Active);
    let expected = record.revision;
    let legacy_native = format!(
        "agent-sandbox:{}",
        record.runtime.as_ref().unwrap().runtime_id
    );
    record.runtime.as_mut().unwrap().runtime_id = legacy_native.clone();
    record.revision += 1;
    harness.store.save_sandbox(expected, record).await.unwrap();
    harness
        .service
        .delete(
            &caller("owner", [Capability::DeleteOwnSandbox]),
            &created.id,
        )
        .await
        .unwrap();
    let restarted = routed.worker(&harness, true, "remote-seed");
    for step in 1..4 {
        restarted
            .run_one(&created.id, now + Duration::from_secs(step * 31))
            .await
            .unwrap();
    }
    let refused = harness.stored(&created.id).await;
    assert_eq!(refused.status, SandboxStatus::Retiring);
    assert_eq!(refused.runtime.as_ref().unwrap().runtime_id, legacy_native);
    assert!(!refused.lifecycle.home.as_ref().unwrap().released);
    assert_eq!(routed.native_deletes.load(Ordering::SeqCst), 0);
    assert_eq!(routed.local.inventory().await.unwrap().len(), 1);
}
