use super::super::startup::{await_ready, observed_ready};
use super::*;

fn fixture() -> (Memory, Provider) {
    let calls = Arc::new(Mutex::new(vec![]));
    let original = target();
    let provider = Provider {
        calls: calls.clone(),
        original: original.plan.clone(),
        refuse_replaced_uid: AtomicBool::new(false),
        retired: AtomicBool::new(false),
    };
    let store = Memory {
        target: Mutex::new(original),
        bind_response_loss: AtomicBool::new(false),
        retirement_block: Mutex::new(std::collections::VecDeque::new()),
        drain_block: Mutex::new(std::collections::VecDeque::new()),
        calls,
    };
    (store, provider)
}

#[tokio::test(start_paused = true)]
async fn startup_adopts_prewarmed_binding_without_zeroing_capacity_before_readiness() {
    let (store, provider) = fixture();
    await_ready(
        &store,
        &provider,
        &provider.original,
        Duration::from_secs(120),
    )
    .await
    .unwrap();
    assert_eq!(
        *store.calls.lock().unwrap(),
        [
            "claim:wp_test",
            "inventory",
            "persist-binding",
            "resize:3",
            "finish:true",
            "inventory",
        ]
    );
    let target = store.target.lock().unwrap().clone();
    assert_eq!(target.binding, Some(provider.binding()));
    assert_eq!(target.applied_revision, target.revision);
    store.calls.lock().unwrap().clear();
    await_ready(
        &store,
        &provider,
        &provider.original,
        Duration::from_secs(120),
    )
    .await
    .unwrap();
    assert_eq!(*store.calls.lock().unwrap(), ["inventory"]);
}

#[tokio::test(start_paused = true)]
async fn startup_respects_durable_zero_target_without_provider_mutation_or_inventory() {
    let (store, provider) = fixture();
    store.target.lock().unwrap().target_replicas = 0;
    assert_eq!(store.target.lock().unwrap().requested.target_replicas, 3);
    await_ready(
        &store,
        &provider,
        &provider.original,
        Duration::from_secs(120),
    )
    .await
    .unwrap();
    assert!(store.calls.lock().unwrap().is_empty());
    assert!(store.target.lock().unwrap().binding.is_none());
}

#[tokio::test(start_paused = true)]
async fn startup_fails_within_deadline_when_target_cannot_be_warmed() {
    let (store, provider) = fixture();
    provider.refuse_replaced_uid.store(true, Ordering::SeqCst);
    let began = tokio::time::Instant::now();
    assert!(matches!(
        await_ready(
            &store,
            &provider,
            &provider.original,
            Duration::from_secs(120)
        )
        .await,
        Err(WarmPoolError::Unavailable),
    ));
    assert_eq!(began.elapsed(), Duration::from_secs(120));
    assert_eq!(store.target.lock().unwrap().applied_revision, 0);
    assert_eq!(store.target.lock().unwrap().target_replicas, 3);
}

#[test]
fn startup_accepts_one_spare_only_for_current_observed_durable_binding() {
    let (_, provider) = fixture();
    let mut target = target();
    target.binding = Some(provider.binding());
    target.applied_revision = target.revision;
    let ready = NativePoolStatus {
        binding: provider.binding(),
        desired_replicas: 3,
        ready_replicas: 1,
        observed: true,
    };
    assert!(observed_ready(&target, std::slice::from_ref(&ready)));
    for changed in 0..5 {
        let mut status = ready.clone();
        match changed {
            0 => status.binding.uid = "replacement".into(),
            1 => status.binding.template_uid = "replacement".into(),
            2 => status.desired_replicas = 2,
            3 => status.ready_replicas = 0,
            _ => status.observed = false,
        }
        assert!(!observed_ready(&target, &[status]));
    }
    target.applied_revision = 0;
    assert!(!observed_ready(&target, std::slice::from_ref(&ready)));
    target.applied_revision = target.revision;
    target.error = Some("unavailable".into());
    assert!(!observed_ready(&target, &[ready]));
}

#[tokio::test(start_paused = true)]
async fn startup_awaits_only_the_default_pool_even_with_a_cold_positive_machine_target() {
    let store = Arc::new(resize::Targets::default());
    let machine_image = resize::pinned("omarchy", '1');
    let pools = resize::deployment(
        store.clone(),
        resize::Planner::default(),
        &resize::pinned("agent", 'a'),
        Some((&machine_image, 3)),
    );
    pools.ensure_default().await.unwrap();
    let machine = resize::only(store.machine(&machine_image));
    assert_eq!(machine.target_replicas, 3);
    let inventory = {
        let mut rows = store.rows.lock().unwrap();
        let default = rows.iter_mut().find(|row| !row.machine).unwrap();
        let binding = resize::Planner::binding(&default.plan);
        default.binding = Some(binding.clone());
        default.applied_revision = default.revision;
        vec![NativePoolStatus {
            binding,
            desired_replicas: default.target_replicas,
            ready_replicas: 1,
            observed: true,
        }]
    };
    let planner = resize::Planner {
        inventory: Mutex::new(inventory),
    };
    // The machine pool is still pulling its image: absent from inventory.
    let pools = resize::deployment(
        store.clone(),
        planner,
        &resize::pinned("agent", 'a'),
        Some((&machine_image, 3)),
    );
    let began = tokio::time::Instant::now();
    pools.await_default_ready().await.unwrap();
    assert_eq!(began.elapsed(), Duration::ZERO);
    assert!(store.claims.lock().unwrap().is_empty());
    let machine = resize::only(store.machine(&machine_image));
    assert_eq!(machine.target_replicas, 3);
    assert!(machine.binding.is_none());
}

#[tokio::test(start_paused = true)]
async fn startup_drain_never_claims_or_zeroes_a_prewarmed_default() {
    // The same prewarmed, unbound default as above, but through the full
    // startup wait with superseded-pool draining running alongside it.
    let (store, mut provider) = fixture();
    let image = crate::warm_pools::deployment_image(provider.original.spec.image.uri.clone());
    provider.original.spec.image = image.clone();
    store.target.lock().unwrap().plan.spec.image = image;
    let store = Arc::new(store);
    let home = provider.original.spec.home.clone();
    let capacity = layer3_adapter::SandboxCapacity {
        cpu_millis: provider.original.spec.cpu_millis,
        memory_mib: provider.original.spec.memory_mib,
    };
    let uri = provider.original.spec.image.uri.clone();
    let adapter = layer3_adapter::codex::CodexAdapter::with_backend(
        resize::PoolBackend(provider),
        layer3_adapter::codex::CodexSandboxSpec::default(),
    );
    let service = NativeWarmPools::new(
        store.clone(),
        Arc::new(adapter),
        uri,
        Some(home),
        capacity,
        1,
        None,
    );
    service.await_default_ready().await.unwrap();
    assert_eq!(
        *store.calls.lock().unwrap(),
        [
            "claim:wp_test",
            "inventory",
            "persist-binding",
            "resize:3",
            "finish:true",
            "inventory",
        ]
    );
}
