use super::*;
use layer3_adapter::codex::{CodexAdapter, CodexSandboxSpec};
use layer3_adapter::hosted::{HostedAgentOpenSettings, HostedBackend};
use layer3_adapter::provider::ProviderSandboxRequest;
use layer3_adapter::wire::{
    AgentOpenReceipt, FencedReceipt, FencedRequest, RequestFrame, ResponseFrame,
};
use layer3_adapter::{AgentLaunch, ConversationChannel, ConversationUpdate, RuntimeHandle};

pub(super) struct PoolBackend(pub(super) Provider);

#[async_trait]
impl HostedBackend for PoolBackend {
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
    service_mut.machine = Some((service_mut.image.clone(), 2));
    service_mut.image =
        deployment_image(format!("registry.example/agent@sha256:{}", "b".repeat(64)));
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
    assert_eq!(*store.calls.lock().unwrap(), ["store-resize:wp_test:1:2"]);
    let mut wrong_channel = original.clone();
    wrong_channel.machine = false;
    assert!(!service.current_plan(&wrong_channel).unwrap());
    let service_mut = Arc::get_mut(&mut service).unwrap();
    service_mut.machine = None;
    assert!(!service.current_plan(&original).unwrap());
}
