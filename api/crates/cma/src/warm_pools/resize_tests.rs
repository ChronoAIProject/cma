use super::*;
use layer3_adapter::codex::{CodexAdapter, CodexSandboxSpec};
use layer3_adapter::hosted::{HostedAgentOpenSettings, HostedBackend};
use layer3_adapter::provider::ProviderSandboxRequest;
use layer3_adapter::wire::{
    AgentOpenReceipt, FencedReceipt, FencedRequest, RequestFrame, ResponseFrame,
};
use layer3_adapter::{AgentLaunch, ConversationChannel, ConversationUpdate, RuntimeHandle};

pub(super) struct PoolBackend<P = Provider>(pub(super) P);

#[async_trait]
impl<P: NativePoolControl + 'static> HostedBackend for PoolBackend<P> {
    fn native_pools(&self) -> Option<&dyn NativePoolControl> {
        Some(&self.0)
    }
    async fn provision(&self, _: ProviderSandboxRequest) -> layer3_adapter::Result<RuntimeHandle> {
        unreachable!("resizing a pool cannot provision an owned runtime")
    }
    async fn exchange(
        &self,
        _: &RuntimeHandle,
        _: RequestFrame,
    ) -> layer3_adapter::Result<ResponseFrame> {
        unreachable!("resizing a pool cannot contact an owned runtime")
    }
    async fn open_agent(
        &self,
        _: &RuntimeHandle,
        _: FencedRequest<AgentLaunch>,
        _: Option<String>,
        _: HostedAgentOpenSettings,
        _: tokio::sync::mpsc::Sender<ConversationUpdate>,
    ) -> layer3_adapter::Result<(ConversationChannel, FencedReceipt<AgentOpenReceipt>)> {
        unreachable!("resizing a pool cannot open an Agent")
    }
    async fn teardown(&self, _: &RuntimeHandle) -> layer3_adapter::Result<()> {
        unreachable!("resizing a pool cannot tear down an owned runtime")
    }
}

fn service(
    original: PoolTarget,
    current: NativePoolConfiguration,
) -> (Arc<NativeWarmPools>, Arc<Memory>) {
    let calls = Arc::new(Mutex::new(vec![]));
    let store = Arc::new(Memory {
        target: Mutex::new(original),
        bind_response_loss: AtomicBool::new(false),
        retirement_block: Mutex::new(std::collections::VecDeque::new()),
        drain_block: Mutex::new(std::collections::VecDeque::new()),
        calls: calls.clone(),
    });
    let adapter = CodexAdapter::with_backend(
        PoolBackend(Provider {
            calls,
            original: current.clone(),
            refuse_replaced_uid: AtomicBool::new(false),
            retired: AtomicBool::new(false),
        }),
        CodexSandboxSpec::default(),
    );
    let service = NativeWarmPools::new(
        store.clone(),
        Arc::new(adapter),
        current.spec.image.uri,
        Some(current.spec.home),
        layer3_adapter::SandboxCapacity {
            cpu_millis: 2000,
            memory_mib: 8192,
        },
        1,
        None,
    );
    (service, store)
}

fn operator() -> Caller {
    Caller::new(
        layer2_core::SubjectId("operator".into()),
        layer2_core::Authorization::new([Capability::ConfigurePlatform]),
    )
}

fn original_target() -> PoolTarget {
    let mut original = target();
    original.plan.spec.image = deployment_image(original.plan.spec.image.uri);
    original
}

#[tokio::test]
async fn stale_nonzero_resize_never_reaches_the_store() {
    let original = original_target();
    let mut next_image = original.plan.clone();
    next_image.spec.image.uri = format!("registry.example/runtime@sha256:{}", "b".repeat(64));
    let mut next_policy = original.plan.clone();
    next_policy.pod_policy.service_account = "current-service".into();
    let mut next_home = original.plan.clone();
    next_home.spec.home.capacity_gib += 1;
    for current in [next_image, next_policy, next_home] {
        let (service, store) = service(original.clone(), current);
        let result = service
            .resize(
                &operator(),
                &original.id,
                ResizeWarmPoolBody {
                    expected_revision: original.revision as u64,
                    target_replicas: 4,
                },
            )
            .await;
        assert!(matches!(result, Err(WarmPoolError::Conflict)));
        assert!(store.calls.lock().unwrap().is_empty());
        let retained = store.target.lock().unwrap();
        assert_eq!(retained.revision, original.revision);
        assert_eq!(retained.target_replicas, original.target_replicas);
        assert_eq!(retained.plan, original.plan);
    }
}

#[tokio::test]
async fn stale_zero_resize_keeps_original_authority_and_reaches_the_store_once() {
    let original = original_target();
    let mut current = original.plan.clone();
    current.spec.image.uri = format!("registry.example/runtime@sha256:{}", "b".repeat(64));
    let (service, store) = service(original.clone(), current);
    let result = service
        .resize(
            &operator(),
            &original.id,
            ResizeWarmPoolBody {
                expected_revision: original.revision as u64,
                target_replicas: 0,
            },
        )
        .await
        .unwrap();
    assert_eq!(result.target_replicas, 0);
    assert_eq!(result.image, original.plan.spec.image.uri);
    assert_eq!(*store.calls.lock().unwrap(), ["store-resize:wp_test:1:0"]);
    let retained = store.target.lock().unwrap();
    assert_eq!(retained.plan, original.plan);
    assert_eq!(retained.binding, original.binding);
    assert_eq!(retained.revision, original.revision + 1);
}

#[tokio::test]
async fn current_pools_with_nondefault_resources_can_be_resized() {
    for (cpu, memory) in [(1000, 1024), (4000, 16384)] {
        let mut original = original_target();
        original.plan.spec.cpu_millis = cpu;
        original.plan.spec.memory_mib = memory;
        original.requested.cpu_millis = cpu;
        original.requested.memory_mib = memory;
        let (service, store) = service(original.clone(), original.plan.clone());
        assert_ne!(cpu, service.default_capacity.cpu_millis);
        assert_ne!(memory, service.default_capacity.memory_mib);
        let result = service
            .resize(
                &operator(),
                &original.id,
                ResizeWarmPoolBody {
                    expected_revision: original.revision as u64,
                    target_replicas: 5,
                },
            )
            .await
            .unwrap();
        assert_eq!((result.cpu_millis, result.memory_mib), (cpu, memory));
        assert_eq!(result.target_replicas, 5);
        assert_eq!(*store.calls.lock().unwrap(), ["store-resize:wp_test:1:5"]);
        assert_eq!(store.target.lock().unwrap().plan, original.plan);
    }
}

#[tokio::test]
async fn machine_pool_survives_reconciliation_beside_a_different_agent_image() {
    let mut original = original_target();
    original.machine = true;
    original.document_schema_version = 2;
    let (mut service, store) = service(original.clone(), original.plan.clone());
    let service_mut = Arc::get_mut(&mut service).unwrap();
    let agent = service_mut.released_images().agent.uri;
    service_mut.set_released(
        format!("registry.example/agent@sha256:{}", "b".repeat(64)),
        Some((agent, 2)),
    );
    assert!(!service_mut.current_plan(&original).unwrap());
    service_mut.default_capacity = layer3_adapter::SandboxCapacity {
        cpu_millis: original.plan.spec.cpu_millis,
        memory_mib: original.plan.spec.memory_mib,
    };
    assert!(service.current_plan(&original).unwrap());
    service
        .resize(
            &operator(),
            &original.id,
            ResizeWarmPoolBody {
                expected_revision: 1,
                target_replicas: 2,
            },
        )
        .await
        .unwrap();
    // A current machine plan's resize is the operator's carried decision.
    assert_eq!(
        *store.calls.lock().unwrap(),
        ["store-resize:wp_test:1:2:choice"]
    );
    let mut wrong_channel = original.clone();
    wrong_channel.machine = false;
    assert!(!service.current_plan(&wrong_channel).unwrap());
    let service_mut = Arc::get_mut(&mut service).unwrap();
    let agent = service_mut.released_images().agent.uri;
    service_mut.set_released(agent, None);
    assert!(!service.current_plan(&original).unwrap());
}

#[tokio::test]
async fn warm_pool_list_names_the_machine_pool_and_its_configured_image() {
    let mut original = original_target();
    original.machine = true;
    original.document_schema_version = 2;
    let (mut service, _) = service(original.clone(), original.plan.clone());
    let service_mut = Arc::get_mut(&mut service).unwrap();
    let agent = service_mut.released_images().agent.uri;
    service_mut.set_released(
        format!("registry.example/agent@sha256:{}", "b".repeat(64)),
        Some((agent, 2)),
    );
    // Which pool is the desktop pool is durable target data, not inventory;
    // without a home policy the list skips both inventory and default planning.
    service_mut.home = None;
    let list = service.list(&operator()).await.unwrap();
    assert!(!list.available);
    assert!(list.items[0].machine);
    assert_eq!(list.items[0].image, original.plan.spec.image.uri);
    assert_eq!(
        list.machine_image.as_deref(),
        Some(original.plan.spec.image.uri.as_str())
    );
    assert_ne!(list.default_image, original.plan.spec.image.uri);
    let service_mut = Arc::get_mut(&mut service).unwrap();
    let agent = service_mut.released_images().agent.uri;
    service_mut.set_released(agent, None);
    let list = service.list(&operator()).await.unwrap();
    assert!(list.items[0].machine);
    assert_eq!(list.machine_image, None);
}

#[tokio::test]
async fn target_fencing_defaults_off_and_preserves_legacy_provider_calls() {
    let original = original_target();
    let (worker, store) = service(original.clone(), original.plan.clone());
    assert!(!worker.target_fencing);
    worker.reconcile_one().await.unwrap();
    let calls = store.calls.lock().unwrap();
    assert!(calls.iter().any(|call| call == "resize:3"));
    assert!(calls.iter().any(|call| call == "finish:true"));
}

#[tokio::test]
async fn enabled_target_fencing_never_falls_back_to_an_unversioned_provider() {
    let original = original_target();
    let (worker, store) = service(original.clone(), original.plan.clone());
    let worker = worker.with_target_fencing(true);
    worker.reconcile_one().await.unwrap();
    let calls = store.calls.lock().unwrap();
    assert!(!calls.iter().any(|call| call.starts_with("resize:")));
    assert!(calls.iter().any(|call| call == "finish:false"));
    assert!(store.target.lock().unwrap().error.is_some());
}

/// Several durable targets with the Mongo store's id, plan and fence rules.
type FindGate = (
    tokio::sync::oneshot::Sender<()>,
    tokio::sync::oneshot::Receiver<bool>,
);

#[derive(Default)]
pub(super) struct Targets {
    pub(super) rows: Mutex<Vec<PoolTarget>>,
    pub(super) claims: Mutex<Vec<String>>,
    pub(super) find_error: AtomicBool,
    pub(super) claim_error: AtomicBool,
    pub(super) list_error_after: std::sync::atomic::AtomicUsize,
    pub(super) retirement_claim_error: AtomicBool,
    pub(super) find_gate: Mutex<Option<FindGate>>,
    clock: std::sync::atomic::AtomicI64,
}

impl Targets {
    pub(super) fn machine(&self, image: &str) -> Vec<PoolTarget> {
        let rows = self.rows.lock().unwrap();
        rows.iter()
            .filter(|row| row.machine && row.plan.spec.image.uri == image)
            .cloned()
            .collect()
    }

    /// Retirement zeroes the count and keeps the tombstone.
    pub(super) fn retire(&self, id: &str) {
        let mut rows = self.rows.lock().unwrap();
        let row = rows.iter_mut().find(|row| row.id == id).unwrap();
        row.retiring = true;
        row.retired = true;
        row.target_replicas = 0;
        row.revision += 1;
    }
}

#[async_trait]
impl PoolTargetStore for Targets {
    async fn activate(&self, _: &str) -> Result<(), WarmPoolError> {
        Ok(())
    }
    async fn list(&self) -> Result<Vec<PoolTarget>, WarmPoolError> {
        if self
            .list_error_after
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                remaining.checked_sub(1)
            })
            == Ok(1)
        {
            return Err(WarmPoolError::Unavailable);
        }
        tokio::task::yield_now().await;
        let rows = self.rows.lock().unwrap();
        Ok(rows.iter().filter(|row| !row.retired).cloned().collect())
    }
    async fn find(&self, id: &str) -> Result<Option<PoolTarget>, WarmPoolError> {
        let gate = self.find_gate.lock().unwrap().take();
        if let Some((entered, release)) = gate {
            entered.send(()).unwrap();
            if !matches!(release.await, Ok(true)) {
                return Err(WarmPoolError::Unavailable);
            }
        }
        let mut rows = self.rows.lock().unwrap();
        if let (true, Some(row)) = (
            self.find_error.swap(false, Ordering::SeqCst),
            rows.iter_mut().find(|row| row.id == id),
        ) {
            row.error = Some("unavailable".into());
        }
        Ok(rows.iter().find(|row| row.id == id).cloned())
    }
    async fn insert(&self, target: PoolTarget) -> Result<PoolTarget, WarmPoolError> {
        tokio::task::yield_now().await;
        let mut rows = self.rows.lock().unwrap();
        if let Some(existing) = rows.iter().find(|row| row.id == target.id) {
            return Ok(existing.clone());
        }
        if rows
            .iter()
            .any(|row| row.plan.config_sha256 == target.plan.config_sha256)
        {
            return Err(WarmPoolError::Conflict);
        }
        rows.push(target.clone());
        Ok(target)
    }
    async fn resize(
        &self,
        id: &str,
        body: &ResizeWarmPoolBody,
        operator_choice: bool,
    ) -> Result<PoolTarget, WarmPoolError> {
        let at = self.clock.fetch_add(1, Ordering::SeqCst) + 1;
        let mut rows = self.rows.lock().unwrap();
        let row = rows
            .iter_mut()
            .find(|row| row.id == id)
            .ok_or(WarmPoolError::NotFound)?;
        if row.revision as u64 != body.expected_revision
            || row.retiring
            || row.retired
            || (operator_choice && !row.machine)
        {
            return Err(WarmPoolError::Conflict);
        }
        row.target_replicas = body.target_replicas;
        row.revision += 1;
        if operator_choice {
            row.operator_replicas = Some(OperatorReplicas {
                replicas: body.target_replicas,
                at: mongodb::bson::DateTime::from_millis(at),
            });
        }
        Ok(row.clone())
    }
    async fn latest_machine_operator_replicas(
        &self,
    ) -> Result<Option<OperatorReplicas>, WarmPoolError> {
        let rows = self.rows.lock().unwrap();
        Ok(rows
            .iter()
            .filter(|row| row.machine)
            .filter_map(|row| row.operator_replicas)
            .max_by_key(|choice| choice.at))
    }
    async fn claim(&self) -> Result<Option<PoolTarget>, WarmPoolError> {
        Ok(None)
    }
    async fn claim_target(&self, id: &str) -> Result<Option<PoolTarget>, WarmPoolError> {
        self.claims.lock().unwrap().push(id.into());
        if self.claim_error.swap(false, Ordering::SeqCst) {
            return Err(WarmPoolError::Unavailable);
        }
        Ok(None)
    }
    async fn claim_retirement(&self, _: &str) -> Result<Option<PoolTarget>, WarmPoolError> {
        if self.retirement_claim_error.swap(false, Ordering::SeqCst) {
            return Err(WarmPoolError::Unavailable);
        }
        Ok(None)
    }
    async fn bind(&self, _: &PoolTarget, _: &NativePoolBinding) -> Result<(), WarmPoolError> {
        unreachable!("seeding never binds provider capacity")
    }
    async fn finish(&self, _: &PoolTarget, _: bool) -> Result<(), WarmPoolError> {
        unreachable!("seeding never reconciles provider capacity")
    }
}

/// Plans any image; its inventory reports only the pools it was given.
#[derive(Clone, Default)]
pub(super) struct Planner {
    pub(super) inventory: Arc<Mutex<Vec<NativePoolStatus>>>,
    pub(super) retiring: Arc<AtomicBool>,
    pub(super) inventory_error: Arc<AtomicBool>,
    pub(super) admission_observations: Arc<std::sync::atomic::AtomicUsize>,
    pub(super) inventory_drift: Arc<AtomicBool>,
    pub(super) bound_drift: Arc<AtomicBool>,
    pub(super) superseded_target: Arc<AtomicBool>,
    pub(super) retirement_calls: Arc<Mutex<Vec<RetirementCall>>>,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum RetirementCall {
    Begin(String),
    CheckDefinitions(Vec<layer3_adapter::HomeVolumeAttachment>),
    Drain(String, NativePoolDrain),
}

impl Planner {
    pub(super) fn binding(plan: &NativePoolConfiguration) -> NativePoolBinding {
        NativePoolBinding {
            spec: plan.spec.clone(),
            pod_policy: plan.pod_policy.clone(),
            name: format!("pool-{}", &plan.config_sha256[..8]),
            uid: format!("uid-{}", &plan.config_sha256[..8]),
            template_name: "template".into(),
            template_uid: "template-uid".into(),
            config_sha256: plan.config_sha256.clone(),
        }
    }
}

#[async_trait]
impl NativePoolControl for Planner {
    async fn begin_retirement(&self, binding: &NativePoolBinding) -> layer3_adapter::Result<()> {
        self.retirement_calls
            .lock()
            .unwrap()
            .push(RetirementCall::Begin(binding.uid.clone()));
        Ok(())
    }
    async fn retire_pool(
        &self,
        _: &NativePoolBinding,
        homes: &[layer3_adapter::HomeVolumeAttachment],
    ) -> layer3_adapter::Result<bool> {
        self.retirement_calls
            .lock()
            .unwrap()
            .push(RetirementCall::CheckDefinitions(homes.to_vec()));
        Ok(false)
    }
    async fn request_spare_drain(
        &self,
        binding: &NativePoolBinding,
        drain: &NativePoolDrain,
    ) -> layer3_adapter::Result<()> {
        self.retirement_calls
            .lock()
            .unwrap()
            .push(RetirementCall::Drain(
                binding.uid.clone(),
                NativePoolDrain {
                    homes: drain.homes.clone(),
                    claims: drain.claims.clone(),
                },
            ));
        Ok(())
    }

    fn plan(&self, spec: &NativePoolSpec) -> layer3_adapter::Result<NativePoolConfiguration> {
        let identity = serde_json::to_vec(spec).expect("spec serializes");
        Ok(NativePoolConfiguration {
            spec: spec.clone(),
            pod_policy: target().plan.pod_policy,
            namespace: "sandboxes".into(),
            deployment: "test".into(),
            config_sha256: format!("{:x}", Sha256::digest(identity)),
        })
    }
    async fn configure_planned(
        &self,
        _: &NativePoolConfiguration,
        _: u32,
    ) -> layer3_adapter::Result<NativePoolStatus> {
        unreachable!("seeding and startup never configure an unawaited pool")
    }
    async fn inventory(&self) -> layer3_adapter::Result<Vec<NativePoolStatus>> {
        if self.inventory_error.swap(false, Ordering::SeqCst) {
            return Err(layer3_adapter::AdapterError::Unavailable(
                "synthetic read failure".into(),
            ));
        }
        if self.inventory_drift.load(Ordering::SeqCst) {
            return Err(layer3_adapter::AdapterError::NativePoolConfigurationDrift);
        }
        Ok(self.inventory.lock().unwrap().clone())
    }
    async fn observe_admitting_pool(
        &self,
        binding: &NativePoolBinding,
        _: &layer3_adapter::agent_sandbox::NativePoolTarget,
        fenced: bool,
    ) -> layer3_adapter::Result<NativePoolStatus> {
        self.admission_observations.fetch_add(1, Ordering::SeqCst);
        if fenced && self.superseded_target.load(Ordering::SeqCst) {
            return Err(layer3_adapter::AdapterError::Rejected(
                "superseded target".into(),
            ));
        }
        if self.retiring.load(Ordering::SeqCst) {
            return Err(layer3_adapter::AdapterError::Rejected(
                "retiring pool".into(),
            ));
        }
        if self.bound_drift.load(Ordering::SeqCst) {
            return Err(layer3_adapter::AdapterError::NativePoolConfigurationDrift);
        }
        self.inventory
            .lock()
            .unwrap()
            .iter()
            .find(|status| &status.binding == binding)
            .cloned()
            .ok_or_else(|| layer3_adapter::AdapterError::Rejected("missing binding".into()))
    }
    async fn resize(
        &self,
        _: &NativePoolBinding,
        _: u32,
    ) -> layer3_adapter::Result<NativePoolStatus> {
        unreachable!("seeding and startup never resize an unawaited pool")
    }
    async fn claim(&self, _: &NativePoolClaimRequest) -> layer3_adapter::Result<NativePoolClaim> {
        unreachable!()
    }
    async fn observe_claim(&self, _: &NativePoolClaim) -> layer3_adapter::Result<NativePoolClaim> {
        unreachable!()
    }
    async fn adopt_home(
        &self,
        _: &NativePoolClaim,
    ) -> layer3_adapter::Result<layer3_adapter::HomeVolumeAttachment> {
        unreachable!()
    }
    async fn plan_cleanup(
        &self,
        _: &NativePoolCleanup,
    ) -> layer3_adapter::Result<NativePoolCleanup> {
        unreachable!()
    }
    async fn cleanup(&self, _: &NativePoolCleanup) -> layer3_adapter::Result<()> {
        unreachable!()
    }
}

pub(super) fn pinned(name: &str, digit: char) -> String {
    format!(
        "registry.example/{name}@sha256:{}",
        digit.to_string().repeat(64)
    )
}

/// One Agent Manager pod: its own planner, deployment images and env targets.
pub(super) fn deployment(
    store: Arc<dyn PoolTargetStore>,
    planner: Planner,
    agent: &str,
    machine: Option<(&str, u32)>,
) -> Arc<NativeWarmPools> {
    NativeWarmPools::new(
        store,
        Arc::new(CodexAdapter::with_backend(
            PoolBackend(planner),
            CodexSandboxSpec::default(),
        )),
        agent.into(),
        Some(target().plan.spec.home),
        layer3_adapter::SandboxCapacity {
            cpu_millis: 2000,
            memory_mib: 8192,
        },
        1,
        machine.map(|(image, replicas)| (image.into(), replicas)),
    )
}

pub(super) async fn resize_to(pools: &NativeWarmPools, target: &PoolTarget, replicas: u32) {
    pools
        .resize(
            &operator(),
            &target.id,
            ResizeWarmPoolBody {
                expected_revision: target.revision as u64,
                target_replicas: replicas,
            },
        )
        .await
        .unwrap();
}

pub(super) fn only(mut rows: Vec<PoolTarget>) -> PoolTarget {
    assert_eq!(rows.len(), 1);
    rows.remove(0)
}

#[tokio::test]
async fn new_machine_release_keeps_the_operators_count_over_the_deployment_value() {
    let store = Arc::new(Targets::default());
    let agent = pinned("agent", 'a');
    let release = |machine: char| {
        deployment(
            store.clone(),
            Planner::default(),
            &agent,
            Some((&pinned("omarchy", machine), 0)),
        )
    };
    let first = release('1');
    first.ensure_default().await.unwrap();
    let seeded = only(store.machine(&pinned("omarchy", '1')));
    // No operator has resized a machine pool: the deployment value applies.
    assert_eq!(seeded.target_replicas, 0);
    assert_eq!(seeded.operator_replicas, None);
    resize_to(&first, &seeded, 2).await;

    let second = release('2');
    second.ensure_default().await.unwrap();
    let promoted = only(store.machine(&pinned("omarchy", '2')));
    assert_eq!(promoted.target_replicas, 2);
    assert_eq!(promoted.requested.target_replicas, 2);
    assert_eq!(promoted.operator_replicas.map(|c| c.replicas), Some(2));
    // Draining the superseded pool is not a new desired count.
    resize_to(&second, &only(store.machine(&pinned("omarchy", '1'))), 0).await;
    store.retire(&seeded.id);

    let third = release('3');
    third.ensure_default().await.unwrap();
    assert_eq!(
        only(store.machine(&pinned("omarchy", '3'))).target_replicas,
        2
    );
    // Restarting the same release reuses its target rather than reseeding it.
    third.ensure_default().await.unwrap();
    assert_eq!(store.machine(&pinned("omarchy", '3')).len(), 1);
}

#[tokio::test]
async fn operator_zero_is_carried_and_the_deployment_value_applies_before_any_resize() {
    let store = Arc::new(Targets::default());
    let agent = pinned("agent", 'a');
    let release = |machine: char| {
        deployment(
            store.clone(),
            Planner::default(),
            &agent,
            Some((&pinned("omarchy", machine), 4)),
        )
    };
    release('1').ensure_default().await.unwrap();
    let seeded = only(store.machine(&pinned("omarchy", '1')));
    assert_eq!(seeded.target_replicas, 4);
    let second = release('2');
    second.ensure_default().await.unwrap();
    let promoted = only(store.machine(&pinned("omarchy", '2')));
    assert_eq!(promoted.target_replicas, 4);
    assert_eq!(promoted.operator_replicas, None);
    resize_to(&second, &promoted, 0).await;
    store.retire(&seeded.id);
    store.retire(&promoted.id);
    release('3').ensure_default().await.unwrap();
    let carried = only(store.machine(&pinned("omarchy", '3')));
    assert_eq!(carried.target_replicas, 0);
    assert_eq!(carried.operator_replicas.map(|c| c.replicas), Some(0));
}

#[tokio::test]
async fn agent_and_machine_pools_never_carry_each_others_count() {
    let store = Arc::new(Targets::default());
    let release = |agent: char, machine: char| {
        deployment(
            store.clone(),
            Planner::default(),
            &pinned("agent", agent),
            Some((&pinned("omarchy", machine), 0)),
        )
    };
    let agent_pool = |image: char| {
        let rows = store.rows.lock().unwrap();
        let mut rows = rows
            .iter()
            .filter(|row| !row.machine && row.plan.spec.image.uri == pinned("agent", image));
        rows.next().cloned().unwrap()
    };
    let first = release('a', '1');
    first.ensure_default().await.unwrap();
    resize_to(&first, &agent_pool('a'), 7).await;
    assert_eq!(agent_pool('a').operator_replicas, None);
    let second = release('a', '2');
    second.ensure_default().await.unwrap();
    let machine = only(store.machine(&pinned("omarchy", '2')));
    assert_eq!(machine.target_replicas, 0);
    resize_to(&second, &machine, 5).await;
    // A new agent runtime keeps its deployment value, never the desktop count.
    release('b', '2').ensure_default().await.unwrap();
    let promoted = agent_pool('b');
    assert_eq!(promoted.target_replicas, 1);
    assert_eq!(promoted.operator_replicas, None);
}

#[tokio::test]
async fn earlier_binary_resize_of_the_newest_machine_release_is_carried() {
    let store = Arc::new(Targets::default());
    let agent = pinned("agent", 'a');
    let release = |machine: char| {
        deployment(
            store.clone(),
            Planner::default(),
            &agent,
            Some((&pinned("omarchy", machine), 0)),
        )
    };
    release('0').ensure_default().await.unwrap();
    release('1').ensure_default().await.unwrap();
    let drained = only(store.machine(&pinned("omarchy", '0')));
    let legacy = only(store.machine(&pinned("omarchy", '1')));
    {
        // An earlier binary advanced revisions without recording a choice:
        // the older release was drained, the newest one resized.
        let mut rows = store.rows.lock().unwrap();
        for row in rows.iter_mut() {
            if row.id == drained.id {
                (row.target_replicas, row.revision) = (0, 2);
                row.activated_at = Some(mongodb::bson::DateTime::from_millis(1));
            } else if row.id == legacy.id {
                (row.target_replicas, row.revision) = (3, 2);
                row.activated_at = Some(mongodb::bson::DateTime::from_millis(2));
            }
        }
    }
    release('2').ensure_default().await.unwrap();
    let promoted = only(store.machine(&pinned("omarchy", '2')));
    assert_eq!(promoted.target_replicas, 3);
    assert_eq!(promoted.operator_replicas.map(|c| c.replicas), Some(3));
    // The copied decision outlives the legacy target's retirement.
    store.retire(&drained.id);
    store.retire(&legacy.id);
    release('3').ensure_default().await.unwrap();
    assert_eq!(
        only(store.machine(&pinned("omarchy", '3'))).target_replicas,
        3
    );
}

#[tokio::test]
async fn concurrent_starters_insert_one_machine_target_with_the_carried_count() {
    let store = Arc::new(Targets::default());
    let agent = pinned("agent", 'a');
    let release = |machine: char| {
        deployment(
            store.clone(),
            Planner::default(),
            &agent,
            Some((&pinned("omarchy", machine), 0)),
        )
    };
    let first = release('1');
    first.ensure_default().await.unwrap();
    resize_to(&first, &only(store.machine(&pinned("omarchy", '1'))), 2).await;
    let (left, right) = (release('2'), release('2'));
    let (left, right) = tokio::join!(left.ensure_default(), right.ensure_default());
    left.unwrap();
    right.unwrap();
    let promoted = only(store.machine(&pinned("omarchy", '2')));
    assert_eq!(promoted.target_replicas, 2);
    assert_eq!(promoted.revision, 1);
}
