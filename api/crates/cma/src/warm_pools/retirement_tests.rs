use super::*;

fn fixture(blocks: &[bool]) -> (Memory, Provider) {
    let calls = Arc::new(Mutex::new(vec![]));
    let mut original = target();
    let provider = Provider {
        calls: calls.clone(),
        original: original.plan.clone(),
        refuse_replaced_uid: AtomicBool::new(false),
        retired: AtomicBool::new(false),
    };
    original.binding = Some(provider.binding());
    let store = Memory {
        target: Mutex::new(original),
        bind_response_loss: AtomicBool::new(false),
        retirement_block: Mutex::new(std::collections::VecDeque::new()),
        drain_block: Mutex::new(blocks.iter().copied().collect()),
        calls,
    };
    (store, provider)
}

async fn reconcile(store: &Memory, provider: &Provider) {
    let original = store.target.lock().unwrap().clone();
    super::super::retirement::retire_claimed(store, provider, "current", original)
        .await
        .unwrap();
}

#[tokio::test]
async fn late_allocation_after_provider_marker_prevents_drain_and_definition_removal() {
    let (store, provider) = fixture(&[false, true]);
    reconcile(&store, &provider).await;
    assert_eq!(
        *store.calls.lock().unwrap(),
        [
            "drain-scan",
            "persist-retiring",
            "mark-provider",
            "scan",
            "drain-scan",
            "finish:false"
        ]
    );
    assert_eq!(
        store.target.lock().unwrap().binding,
        Some(provider.binding())
    );
}

#[tokio::test]
async fn pending_allocation_prevents_admission_marker_or_any_provider_mutation() {
    let (store, provider) = fixture(&[true]);
    reconcile(&store, &provider).await;
    assert_eq!(*store.calls.lock().unwrap(), ["drain-scan", "finish:false"]);
    assert!(!store.target.lock().unwrap().retiring);
}

#[tokio::test]
async fn retirement_drains_before_removal_and_replays_without_recreating_definitions() {
    let (store, provider) = fixture(&[]);
    reconcile(&store, &provider).await;
    assert_eq!(
        *store.calls.lock().unwrap(),
        [
            "drain-scan",
            "persist-retiring",
            "mark-provider",
            "scan",
            "drain-scan",
            "prove-retirement",
            "prove-drain",
            "resize:0",
            "finish:true"
        ]
    );
    store.calls.lock().unwrap().clear();
    provider.retired.store(true, Ordering::SeqCst);
    reconcile(&store, &provider).await;
    assert_eq!(
        *store.calls.lock().unwrap(),
        [
            "drain-scan",
            "persist-retiring",
            "mark-provider",
            "scan",
            "drain-scan",
            "prove-retirement",
            "remove-target"
        ]
    );
}

fn service(store: Arc<Memory>, provider: Provider) -> Arc<NativeWarmPools> {
    let home = provider.original.spec.home.clone();
    let adapter = layer3_adapter::codex::CodexAdapter::with_backend(
        resize::PoolBackend(provider),
        layer3_adapter::codex::CodexSandboxSpec::default(),
    );
    NativeWarmPools::new(
        store,
        Arc::new(adapter),
        format!("registry.example/runtime@sha256:{}", "b".repeat(64)),
        Some(home),
        layer3_adapter::SandboxCapacity {
            cpu_millis: 2000,
            memory_mib: 8192,
        },
        1,
        None,
    )
}

#[tokio::test]
async fn actual_reconciler_drains_obsolete_zero_and_positive_targets_while_retained_home_blocks_delete(
) {
    for explicit_zero in [false, true] {
        let (store, provider) = fixture(&[]);
        store.retirement_block.lock().unwrap().extend([true, true]);
        let original = store.target.lock().unwrap().clone();
        let store = Arc::new(store);
        let service = service(store.clone(), provider);
        if explicit_zero {
            let operator = Caller::new(
                layer2_core::SubjectId("operator".into()),
                layer2_core::Authorization::new([Capability::ConfigurePlatform]),
            );
            let receipt = service
                .resize(
                    &operator,
                    &original.id,
                    ResizeWarmPoolBody {
                        expected_revision: original.revision as u64,
                        target_replicas: 0,
                    },
                )
                .await
                .unwrap();
            assert_eq!(receipt.target_replicas, 0);
            assert_eq!(receipt.image, original.plan.spec.image.uri);
            assert_eq!(*store.calls.lock().unwrap(), ["store-resize:wp_test:1:0"]);
            // The previous reconciler could acknowledge this zero revision
            // without changing provider capacity. Periodic repair must retry it.
            let mut target = store.target.lock().unwrap();
            target.applied_revision = target.revision;
            store.calls.lock().unwrap().clear();
        }
        for _ in 0..2 {
            assert!(service.reconcile_one().await.unwrap());
            assert_eq!(
                *store.calls.lock().unwrap(),
                [
                    "drain-scan",
                    "persist-retiring",
                    "mark-provider",
                    "scan",
                    "drain-scan",
                    "prove-drain",
                    "resize:0",
                    "finish:true",
                ]
            );
            let target = store.target.lock().unwrap();
            assert_eq!(target.target_replicas, 0);
            assert_eq!(target.plan, original.plan);
            assert_eq!(target.binding, original.binding);
            assert_eq!(target.applied_revision, target.revision);
            assert!(!target.retired);
            store.calls.lock().unwrap().clear();
        }
    }
}

#[tokio::test]
async fn actual_reconciler_keeps_replaced_uid_failure_unapplied_and_preserves_original_binding() {
    let (store, provider) = fixture(&[]);
    provider.refuse_replaced_uid.store(true, Ordering::SeqCst);
    store.retirement_block.lock().unwrap().push_back(true);
    let original = store.target.lock().unwrap().clone();
    let store = Arc::new(store);
    let service = service(store.clone(), provider);
    assert!(service.reconcile_one().await.unwrap());
    assert_eq!(
        *store.calls.lock().unwrap(),
        [
            "drain-scan",
            "persist-retiring",
            "mark-provider",
            "scan",
            "drain-scan",
            "prove-drain",
            "resize:0",
            "finish:false",
        ]
    );
    let target = store.target.lock().unwrap();
    assert_eq!(target.applied_revision, original.applied_revision);
    assert_eq!(target.binding, original.binding);
    assert_eq!(target.plan, original.plan);
    assert!(target.error.is_some());
}

#[tokio::test]
async fn lost_binding_ack_retries_only_zero_before_the_admission_barrier() {
    let (store, provider) = fixture(&[]);
    store.target.lock().unwrap().binding = None;
    store.bind_response_loss.store(true, Ordering::SeqCst);
    reconcile(&store, &provider).await;
    assert_eq!(
        *store.calls.lock().unwrap(),
        [
            "drain-scan",
            "configure:0",
            "persist-binding",
            "finish:false",
        ]
    );
    store.calls.lock().unwrap().clear();
    store.retirement_block.lock().unwrap().push_back(true);
    reconcile(&store, &provider).await;
    assert_eq!(
        *store.calls.lock().unwrap(),
        [
            "drain-scan",
            "persist-retiring",
            "mark-provider",
            "scan",
            "drain-scan",
            "prove-drain",
            "resize:0",
            "finish:true",
        ]
    );
}
