//! Native proof and durable fencing inside the production concurrent startup barrier.
//! Registered below binding_authority_tests to share its real Kubernetes fixture once.
use super::*;
use layer2_core::sandbox::store::SandboxStore;
use layer3_adapter::agent_sandbox::{
    AgentSandboxClient, AgentSandboxConfig, NativeFsGroupChangePolicy, NativeHomeVolumes,
};
use layer3_adapter::hosted::AgentSandboxBackend;
use std::sync::atomic::AtomicBool;
use tokio::sync::Semaphore;

#[path = "startup_native_order_backend.rs"]
mod backend;
#[path = "startup_native_order_store.rs"]
mod store;

struct Control {
    api: Arc<Mutex<api::ApiState>>,
    status_gate: Semaphore,
    status_entered: Notify,
    hold_next_home_check: AtomicBool,
    home_check_gate: Semaphore,
    home_check_entered: Notify,
    other_gate: Semaphore,
    other_entered: Notify,
    other_started: AtomicUsize,
    other_settled: AtomicUsize,
    other_active: AtomicUsize,
    background_sweep: AtomicBool,
    cancelled: AtomicUsize,
    cleanup: AtomicUsize,
    fenced: Notify,
    refuse_status: bool,
}
impl Control {
    fn new(api: Arc<Mutex<api::ApiState>>, refuse_status: bool) -> Arc<Self> {
        Arc::new(Self {
            api,
            status_gate: Semaphore::new(0),
            status_entered: Notify::new(),
            hold_next_home_check: AtomicBool::new(false),
            home_check_gate: Semaphore::new(0),
            home_check_entered: Notify::new(),
            other_gate: Semaphore::new(0),
            other_entered: Notify::new(),
            other_started: AtomicUsize::new(0),
            other_settled: AtomicUsize::new(0),
            other_active: AtomicUsize::new(0),
            background_sweep: AtomicBool::new(false),
            cancelled: AtomicUsize::new(0),
            cleanup: AtomicUsize::new(0),
            fenced: Notify::new(),
            refuse_status,
        })
    }
    fn record(&self, event: &str, id: &str) {
        self.api
            .lock()
            .unwrap()
            .calls
            .push((event.into(), id.into()));
    }
    async fn siblings_started(&self, expected: usize) {
        while self.other_started.load(Ordering::SeqCst) < expected {
            self.other_entered.notified().await;
        }
        assert_eq!(self.other_started.load(Ordering::SeqCst), expected);
    }
    fn assert_no_cleanup(&self) {
        assert_eq!(self.cleanup.load(Ordering::SeqCst), 0);
        assert!(!self
            .api
            .lock()
            .unwrap()
            .calls
            .iter()
            .any(|(kind, _)| kind == "HOME_PHASE"));
    }
    fn assert_siblings_settled(&self) {
        let settled = self.other_settled.load(Ordering::SeqCst);
        if self.background_sweep.load(Ordering::SeqCst) {
            assert!(settled >= 8);
        } else {
            assert_eq!(settled, 8);
        }
        assert_eq!(self.other_active.load(Ordering::SeqCst), 0);
    }
}

struct NativeStartup {
    native: fixture::NativeFixture,
    reconciler: SandboxReconciler,
    store: Arc<store::Store>,
    control: Arc<Control>,
    original: Sandbox,
    now: SystemTime,
}
impl NativeStartup {
    async fn new(record_boot: bool, refuse_status: bool) -> Self {
        Self::with_fs_group_policy(record_boot, refuse_status, None).await
    }

    async fn with_fs_group_policy(
        record_boot: bool,
        refuse_status: bool,
        policy: Option<NativeFsGroupChangePolicy>,
    ) -> Self {
        Self::with_default_image(record_boot, refuse_status, policy, None).await
    }

    async fn with_default_image(
        record_boot: bool,
        refuse_status: bool,
        policy: Option<NativeFsGroupChangePolicy>,
        released_image: Option<String>,
    ) -> Self {
        let (original, mut active, now) = active_fixture().await;
        let native = fixture::NativeFixture::with_fs_group_policy(&mut active, policy).await;
        active.revision = 1;
        if record_boot {
            active.runtime.as_mut().unwrap().boot_id = None;
        }
        active.validate().unwrap();
        let control = Control::new(native.api.state.clone(), refuse_status);
        let store = Arc::new(store::Store {
            inner: InMemorySandboxStore::default(),
            control: control.clone(),
            native: active.id.clone(),
        });
        store.inner.create_sandbox(active.clone()).await.unwrap();
        let mut config = CodexSandboxSpec::default();
        config.deployment.image.uri = active.runtime.as_ref().unwrap().runtime_version.clone();
        config.deployment.image.labels.insert(
            layer3_adapter::runtime_bootstrap::RUNTIME_BOOTSTRAP_LABEL.into(),
            "1".into(),
        );
        config.deployment.image.labels.insert(
            layer3_adapter::runtime_bootstrap::RUNTIME_HOME_LAYOUT_LABEL.into(),
            "2".into(),
        );
        let other = InMemoryHostedBackend::default();
        let provisioning = CodexAdapter::with_backend(other.clone(), config.clone());
        for index in 0..8 {
            let mut sibling = active.clone();
            sibling.id = SandboxId(format!("sbx_zz_startup_{index}"));
            assert!(
                active.id < sibling.id,
                "native must occupy the first admission wave"
            );
            sibling.name = format!("startup-sibling-{index}");
            sibling.lifecycle.home = None;
            sibling.runtime.as_mut().unwrap().boot_id = None;
            let handle = provisioning
                .provision_runtime(&layer3_adapter::RuntimeProvisionRequest::new(
                    1,
                    crate::sandbox::runtime_mapping::sandbox_spec(&sibling, "test").unwrap(),
                ))
                .await
                .unwrap();
            sibling.runtime.as_mut().unwrap().runtime_id = handle.0;
            sibling.validate().unwrap();
            store.inner.create_sandbox(sibling).await.unwrap();
        }
        let native_config = AgentSandboxConfig {
            namespace: "cma-test".into(),
            deployment: "test".into(),
            runtime_class: None,
            service_account: "cma-runtime".into(),
            image_pull_secrets: Vec::new(),
            template_network_policy: None,
            template_fs_group_change_policy: policy,
            native_claim_guard_enabled: false,
            runtime_startup_breaker_enabled: false,
            control_key: "fixture".repeat(8),
        };
        let client =
            AgentSandboxClient::new(native.api.client.clone(), native_config.clone(), "fixture")
                .unwrap();
        let homes = Arc::new(
            NativeHomeVolumes::new(native.api.client.clone(), native_config, "fixture").unwrap(),
        );
        if let Some(image) = &released_image {
            config.deployment.image.uri.clone_from(image);
        }
        let adapter = Arc::new(
            CodexAdapter::with_backend(
                backend::Backend {
                    native: AgentSandboxBackend::new(
                        client,
                        Arc::new(backend::Transport(control.clone())),
                    ),
                    other,
                    control: control.clone(),
                },
                config,
            )
            .with_home_volumes(Arc::new(backend::Homes {
                inner: homes,
                control: control.clone(),
            })),
        );
        let inventory = adapter.runtime_inventory().await.unwrap();
        let authority = store
            .inner
            .sandbox_runtime_authority(&layer2_core::sandbox::store::PageRequest {
                limit: 100,
                cursor: None,
            })
            .await
            .unwrap();
        assert_eq!(inventory.len(), 9);
        assert_eq!(authority.items.len(), 9);
        let authority = authority
            .items
            .into_iter()
            .map(|row| (row.id.clone(), row))
            .collect();
        let mut reconciler = SandboxReconciler::new(
            store.clone(),
            adapter,
            Arc::new(original.harness.policy.clone()),
            "test".into(),
        );
        if let Some(image) = released_image {
            reconciler = reconciler.with_runtime_version(image);
        }
        assert_eq!(
            reconciler.startup_binding_schedule(&authority, &inventory, true),
            (8, "none")
        );
        {
            let mut api = native.api.state.lock().unwrap();
            api.calls.clear();
            api.assignments.clear();
            api.assignment_boundaries.clear();
        }
        Self {
            native,
            reconciler,
            store,
            control,
            original: active,
            now,
        }
    }

    fn assert_native_order(&self, fence: &str) {
        let api = self.native.api.state.lock().unwrap();
        assert_eq!(
            api.assignments.len(),
            1,
            "fresh backend must acknowledge one native assignment"
        );
        let assigned = api.assignment_boundaries[0];
        let position = |event: &str| {
            api.calls
                .iter()
                .position(|(kind, _)| kind == event)
                .unwrap()
        };
        let bound = position("NATIVE_BIND");
        let status = position("STATUS_ENTER");
        let returned = position("STATUS_RETURN");
        let durable = position(fence);
        assert!(assigned <= bound && bound < status && status < returned && returned < durable);
        // These are real Kubernetes reads around assignment delivery and after
        // the held status reply, not a synthetic closure's phase labels.
        let pods = api::POD.rsplit_once('/').unwrap().0;
        for path in [&self.native.claim_path, api::MEMBER, pods, api::VOLUME] {
            for (start, end) in [(0, assigned), (assigned, bound), (returned + 1, durable)] {
                assert!(
                    api.calls[start..end]
                        .iter()
                        .any(|(method, found)| method == "GET" && found == path),
                    "missing native proof of {path} between {start} and {end}: {:?}",
                    api.calls
                );
            }
        }
    }
}

// Timeouts bound a broken fixture; every ordering decision uses a gate/event.
async fn until<T>(
    startup: &mut std::pin::Pin<&mut impl std::future::Future<Output = T>>,
    signal: impl std::future::Future<Output = ()>,
) {
    tokio::select! {
        biased;
        _ = signal => {},
        _ = startup => panic!("startup returned while a required operation was held"),
        _ = tokio::time::sleep(Duration::from_secs(10)) => panic!("startup boundary was not reached"),
    }
}

#[tokio::test]
async fn startup_native_proof_status_and_durable_fence_keep_order_with_held_siblings() {
    for record_boot in [false, true] {
        let fixture = NativeStartup::new(record_boot, false).await;
        let startup = fixture.reconciler.initialize_before_workers(fixture.now);
        tokio::pin!(startup);
        until(&mut startup, fixture.control.status_entered.notified()).await;
        until(&mut startup, fixture.control.siblings_started(7)).await;
        fixture.control.assert_no_cleanup();
        fixture.control.status_gate.add_permits(1);
        until(&mut startup, fixture.control.fenced.notified()).await;
        until(&mut startup, fixture.control.siblings_started(8)).await;
        fixture.control.assert_no_cleanup();
        fixture.assert_native_order(if record_boot {
            "DURABLE_CAS"
        } else {
            "DURABLE_READ"
        });
        let stored = fixture
            .store
            .inner
            .get_sandbox(&fixture.original.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            stored.revision,
            fixture.original.revision + u64::from(record_boot)
        );
        assert_eq!(
            stored.runtime.as_ref().unwrap().boot_id.as_deref(),
            Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
        );
        fixture.control.other_gate.add_permits(8);
        tokio::time::timeout(Duration::from_secs(10), startup)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(fixture.control.other_settled.load(Ordering::SeqCst), 8);
        assert_eq!(fixture.control.cancelled.load(Ordering::SeqCst), 0);
        assert!(fixture.control.cleanup.load(Ordering::SeqCst) > 0);
        assert!(fixture
            .control
            .api
            .lock()
            .unwrap()
            .calls
            .iter()
            .any(|(kind, _)| kind == "HOME_PHASE"));
    }
}

#[path = "startup_native_pool_drift.rs"]
mod pool_drift;

#[path = "startup_admitted_spec.rs"]
mod admitted_spec;

#[path = "startup_native_conflict.rs"]
mod conflicts;

#[tokio::test]
async fn startup_native_false_is_nonfatal_and_defers_cleanup_with_a_binding_hold() {
    let fixture = NativeStartup::new(false, true).await;
    // A deliberately future caller floor makes the fresh provider observation
    // stale for recovery. The production binding result is Ok(false), with no
    // recovery mutation or invented success for the refused installation state.
    let floor = SystemTime::now() + Duration::from_secs(3600);
    let startup = fixture.reconciler.initialize_before_workers(floor);
    tokio::pin!(startup);
    until(&mut startup, fixture.control.status_entered.notified()).await;
    until(&mut startup, fixture.control.siblings_started(7)).await;
    fixture.control.status_gate.add_permits(2);
    until(&mut startup, fixture.control.siblings_started(8)).await;
    fixture.control.assert_no_cleanup();
    {
        let api = fixture.control.api.lock().unwrap();
        assert_eq!(
            api.calls
                .iter()
                .filter(|(kind, _)| kind == "STATUS_RETURN")
                .count(),
            2
        );
        assert!(!api
            .calls
            .iter()
            .any(|(kind, _)| matches!(kind.as_str(), "DURABLE_READ" | "DURABLE_CAS")));
    }
    assert_eq!(
        fixture
            .store
            .inner
            .get_sandbox(&fixture.original.id)
            .await
            .unwrap(),
        Some(fixture.original.clone())
    );
    fixture.control.other_gate.add_permits(8);
    tokio::time::timeout(Duration::from_secs(10), startup)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fixture.control.other_settled.load(Ordering::SeqCst), 8);
    assert_eq!(fixture.control.cancelled.load(Ordering::SeqCst), 0);
    fixture.control.assert_no_cleanup();
    assert!(fixture.reconciler.binding_is_held(&fixture.original.id));
    assert_eq!(
        fixture
            .store
            .inner
            .get_sandbox(&fixture.original.id)
            .await
            .unwrap(),
        Some(fixture.original)
    );
}
