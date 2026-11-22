use super::super::startup::{await_ready, observed_ready, STARTUP_WARMUP_LIMIT};
use super::*;
use std::io::Write;

#[derive(Clone, Default)]
struct Output(Arc<Mutex<Vec<u8>>>);
impl Write for Output {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Output {
    type Writer = Self;
    fn make_writer(&'a self) -> Self {
        self.clone()
    }
}

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
    assert_eq!(
        await_ready(
            &store,
            &provider,
            &provider.original,
            Duration::from_secs(120),
        )
        .await,
        Ok(StartupReadiness::Ready),
    );
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
    assert_eq!(
        await_ready(
            &store,
            &provider,
            &provider.original,
            Duration::from_secs(120),
        )
        .await,
        Ok(StartupReadiness::Ready),
    );
    assert_eq!(*store.calls.lock().unwrap(), ["inventory"]);
}

#[tokio::test(start_paused = true)]
async fn startup_respects_durable_zero_target_without_provider_mutation_or_inventory() {
    let (store, provider) = fixture();
    store.target.lock().unwrap().target_replicas = 0;
    assert_eq!(store.target.lock().unwrap().requested.target_replicas, 3);
    assert_eq!(
        await_ready(
            &store,
            &provider,
            &provider.original,
            Duration::from_secs(120),
        )
        .await,
        Ok(StartupReadiness::Ready),
    );
    assert!(store.calls.lock().unwrap().is_empty());
    assert!(store.target.lock().unwrap().binding.is_none());
}

#[test]
fn startup_wait_never_outlasts_the_release_settle_window() {
    assert!(STARTUP_WARMUP_LIMIT <= RELEASE_SETTLE);
}

#[tokio::test(start_paused = true)]
async fn startup_degrades_at_deadline_when_target_cannot_be_warmed() {
    let (store, provider) = fixture();
    provider.refuse_replaced_uid.store(true, Ordering::SeqCst);
    let output = Output::default();
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .without_time()
        .with_writer(output.clone())
        .finish();
    let began = tokio::time::Instant::now();
    let readiness = {
        let _default = tracing::subscriber::set_default(subscriber);
        await_ready(
            &store,
            &provider,
            &provider.original,
            Duration::from_secs(120),
        )
        .await
    };
    assert_eq!(readiness, Ok(StartupReadiness::Degraded));
    assert_eq!(began.elapsed(), Duration::from_secs(120));
    // The warning separates a refused, unapplied target from slow capacity.
    let text = String::from_utf8(output.0.lock().unwrap().clone()).unwrap();
    let warning = text
        .lines()
        .find(|row| row.contains("not ready within startup limit"))
        .unwrap_or_else(|| panic!("{text}"));
    for field in [
        format!("config_sha256={}", provider.original.config_sha256),
        "elapsed_ms=120000".into(),
        "target_recorded=true".into(),
        "target_bound=true".into(),
        "revision_applied=false".into(),
        "target_error=true".into(),
    ] {
        assert!(warning.contains(&field), "{field} missing from {warning}");
    }
    // Degrading leaves the durable target for the supervisor to keep filling.
    assert_eq!(store.target.lock().unwrap().applied_revision, 0);
    assert_eq!(store.target.lock().unwrap().target_replicas, 3);
    assert!(!store.target.lock().unwrap().retiring);
}

#[tokio::test(start_paused = true)]
async fn startup_refuses_a_retiring_or_retired_default_without_waiting() {
    for retired in [false, true] {
        let (store, provider) = fixture();
        {
            let mut target = store.target.lock().unwrap();
            target.retiring = true;
            target.retired = retired;
        }
        let began = tokio::time::Instant::now();
        assert_eq!(
            await_ready(
                &store,
                &provider,
                &provider.original,
                Duration::from_secs(120)
            )
            .await,
            Err(WarmPoolError::Conflict),
        );
        assert_eq!(began.elapsed(), Duration::ZERO);
        assert!(store.calls.lock().unwrap().is_empty());
    }
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
    assert_eq!(
        pools.await_default_ready().await,
        Ok(StartupReadiness::Ready)
    );
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
    assert_eq!(
        service.await_default_ready().await,
        Ok(StartupReadiness::Ready)
    );
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

#[tokio::test(start_paused = true)]
async fn startup_refuses_an_unusable_default_plan_without_waiting() {
    // Without a home policy no default plan exists to wait for; this stays an
    // error rather than a degraded start.
    let (store, provider) = fixture();
    let store = Arc::new(store);
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
        None,
        capacity,
        1,
        None,
    );
    let began = tokio::time::Instant::now();
    assert_eq!(
        service.await_default_ready().await,
        Err(WarmPoolError::Unavailable)
    );
    assert_eq!(began.elapsed(), Duration::ZERO);
    assert!(store.calls.lock().unwrap().is_empty());
}

#[tokio::test(start_paused = true)]
async fn supervisor_fills_the_default_pool_after_a_degraded_start() {
    let (store, provider) = fixture();
    provider.refuse_replaced_uid.store(true, Ordering::SeqCst);
    assert_eq!(
        await_ready(
            &store,
            &provider,
            &provider.original,
            Duration::from_secs(120)
        )
        .await,
        Ok(StartupReadiness::Degraded),
    );
    {
        let target = store.target.lock().unwrap();
        assert!(target.binding.is_some() && target.error.is_some());
        assert_ne!(target.applied_revision, target.revision);
    }
    // Once the cause clears, the supervisor's ordinary reconcile applies the
    // recorded target on its original binding, and the plan reports ready.
    provider.refuse_replaced_uid.store(false, Ordering::SeqCst);
    store.calls.lock().unwrap().clear();
    assert!(reconcile_one(&store, &provider).await.unwrap());
    assert_eq!(*store.calls.lock().unwrap(), ["resize:3", "finish:true"]);
    let target = store.target.lock().unwrap().clone();
    assert_eq!(target.binding, Some(provider.binding()));
    assert!(observed_ready(
        &target,
        &provider.inventory().await.unwrap()
    ));
}

#[tokio::test(start_paused = true)]
async fn degraded_start_is_noted_once_when_its_plan_first_reports_ready() {
    let (store, mut provider) = fixture();
    let image = crate::warm_pools::deployment_image(provider.original.spec.image.uri.clone());
    provider.original.spec.image = image.clone();
    store.target.lock().unwrap().plan.spec.image = image;
    provider.refuse_replaced_uid.store(true, Ordering::SeqCst);
    let inventory = [NativePoolStatus {
        binding: provider.binding(),
        desired_replicas: 3,
        ready_replicas: 1,
        observed: true,
    }];
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
    assert_eq!(
        service.await_default_ready().await,
        Ok(StartupReadiness::Degraded)
    );
    assert!(service.degraded_start.lock().unwrap().is_some());
    // An erroring, unapplied target is not ready even with a ready member.
    let failing = store.target.lock().unwrap().clone();
    service.note_default_recovery(std::slice::from_ref(&failing), &inventory);
    assert!(service.degraded_start.lock().unwrap().is_some());
    let mut ready = failing;
    ready.error = None;
    ready.applied_revision = ready.revision;
    service.note_default_recovery(std::slice::from_ref(&ready), &inventory);
    assert!(service.degraded_start.lock().unwrap().is_none());
}
