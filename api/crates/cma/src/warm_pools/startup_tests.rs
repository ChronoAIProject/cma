use super::super::startup::{
    await_ready, await_ready_with_fencing, observed_capacity, observed_ready, StartupPoolPolicy,
    WarmPoolStartup,
};
use super::*;
use crate::startup::Admission;

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
async fn empty_startup_requires_opt_in_exact_admission_and_fresh_durable_target() {
    let store = Arc::new(resize::Targets::default());
    let planner = resize::Planner::default();
    let image = resize::pinned("agent", 'a');
    let pools = resize::deployment(store.clone(), planner.clone(), &image, None);
    pools.ensure_default().await.unwrap();
    let original = {
        let mut rows = store.rows.lock().unwrap();
        let row = rows.first_mut().unwrap();
        row.target_replicas = 2;
        row.applied_revision = row.revision;
        row.binding = Some(resize::Planner::binding(&row.plan));
        row.clone()
    };
    let ready = NativePoolStatus {
        binding: original.binding.clone().unwrap(),
        desired_replicas: 2,
        ready_replicas: 0,
        observed: true,
    };
    planner.inventory.lock().unwrap().push(ready.clone());
    let limit = Duration::from_secs(3);
    let policy = StartupPoolPolicy {
        allow_empty: true,
        ..Default::default()
    };
    assert!(matches!(
        await_ready(&*store, &planner, &original.plan, limit).await,
        Err(WarmPoolError::Unavailable)
    ));
    let enabled = pools.with_empty_pool_startup(true);
    let started = tokio::time::Instant::now();
    let boot = enabled
        .await_default_ready_until(Some(&Admission::new()))
        .await
        .unwrap();
    assert!(matches!(boot, WarmPoolStartup::CapacityDeferred(proof) if proof.desired() == 2));
    assert_eq!(started.elapsed(), Duration::from_secs(240));
    let stored = store.find(&original.id).await.unwrap().unwrap();
    assert_eq!(stored.target_replicas, 2);
    assert_eq!(stored.binding, original.binding);
    assert_eq!(stored.revision, original.revision);
    assert_eq!(stored.view(Some(&ready), true).status, "pending");
    assert!(stored
        .view(Some(&ready), true)
        .error
        .unwrap()
        .contains("degraded"));
    assert!(enabled
        .candidate_inventory(&original)
        .await
        .unwrap()
        .is_none());

    assert!(matches!(
        await_ready_with_fencing(
            &*store,
            &planner,
            &original.plan,
            tokio::time::Instant::now() + limit,
            policy,
            None,
            None,
        )
        .await,
        Err(WarmPoolError::Unavailable)
    ));
    for missing in [true, false] {
        let rows = if missing {
            vec![]
        } else {
            let mut duplicate = original.clone();
            duplicate.id = "duplicate-target".into();
            vec![original.clone(), duplicate]
        };
        *store.rows.lock().unwrap() = rows;
        assert!(matches!(
            enabled
                .await_default_ready_until(Some(&Admission::new()))
                .await,
            Err(WarmPoolError::Unavailable)
        ));
        *store.rows.lock().unwrap() = vec![original.clone()];
    }

    for retirement_read in [false, true] {
        let observations = planner.admission_observations.load(Ordering::SeqCst);
        let claims = store.claims.lock().unwrap().len();
        if retirement_read {
            // The readiness and retirement branches list in that order.
            store.list_error_after.store(2, Ordering::SeqCst);
        } else {
            planner.inventory_error.store(true, Ordering::SeqCst);
        }
        let started = tokio::time::Instant::now();
        assert!(matches!(
            enabled
                .await_default_ready_until(Some(&Admission::new()))
                .await,
            Ok(WarmPoolStartup::CapacityDeferred(_))
        ));
        assert_eq!(started.elapsed(), Duration::from_secs(240));
        assert_eq!(
            planner.admission_observations.load(Ordering::SeqCst),
            observations + 1
        );
        assert_eq!(store.list_error_after.load(Ordering::SeqCst), 0);
        assert!(!planner.inventory_error.load(Ordering::SeqCst));
        assert_eq!(
            store.claims.lock().unwrap().len() - claims,
            if retirement_read { 240 } else { 239 },
        );
    }

    let observations = planner.admission_observations.load(Ordering::SeqCst);
    store.claim_error.store(true, Ordering::SeqCst);
    assert!(matches!(
        enabled
            .await_default_ready_until(Some(&Admission::new()))
            .await,
        Err(WarmPoolError::Unavailable)
    ));
    assert!(!store.claim_error.load(Ordering::SeqCst));
    assert_eq!(
        planner.admission_observations.load(Ordering::SeqCst),
        observations
    );

    let mut superseded = original.clone();
    superseded.id = "superseded".into();
    superseded.retiring = true;
    store.rows.lock().unwrap().push(superseded);
    store.retirement_claim_error.store(true, Ordering::SeqCst);
    assert!(matches!(
        enabled
            .await_default_ready_until(Some(&Admission::new()))
            .await,
        Err(WarmPoolError::Unavailable)
    ));
    assert!(!store.retirement_claim_error.load(Ordering::SeqCst));
    assert_eq!(
        planner.admission_observations.load(Ordering::SeqCst),
        observations
    );
    store.rows.lock().unwrap().truncate(1);

    for case in ["resize", "drain", "read_failed", "timeout"] {
        let (entered, receiving) = tokio::sync::oneshot::channel();
        let (release, held) = tokio::sync::oneshot::channel();
        let mut release = Some(release);
        *store.find_gate.lock().unwrap() = Some((entered, held));
        let admission = Admission::new();
        let started = tokio::time::Instant::now();
        let (result, ()) =
            tokio::join!(enabled.await_default_ready_until(Some(&admission)), async {
                receiving.await.unwrap();
                match case {
                    "resize" => {
                        store.rows.lock().unwrap()[0].revision += 1;
                        release.take().unwrap().send(true).unwrap();
                    }
                    "drain" => admission.close(),
                    "read_failed" => release.take().unwrap().send(false).unwrap(),
                    _ => {}
                }
            },);
        if case == "drain" {
            assert!(matches!(result, Ok(WarmPoolStartup::Stopping)));
        } else {
            assert!(
                matches!(result, Err(WarmPoolError::Unavailable)),
                "{case}: {result:?}"
            );
        }
        let expected = if case == "timeout" { 248 } else { 240 };
        assert_eq!(started.elapsed(), Duration::from_secs(expected), "{case}");
        store.rows.lock().unwrap()[0] = original.clone();
    }

    planner.retiring.store(true, Ordering::SeqCst);
    assert!(matches!(
        await_ready_with_fencing(
            &*store,
            &planner,
            &original.plan,
            tokio::time::Instant::now() + limit,
            policy,
            Some(&Admission::new()),
            None
        )
        .await,
        Err(WarmPoolError::Conflict)
    ));
    planner.retiring.store(false, Ordering::SeqCst);
    store.find_error.store(true, Ordering::SeqCst);
    assert!(matches!(
        await_ready_with_fencing(
            &*store,
            &planner,
            &original.plan,
            tokio::time::Instant::now() + limit,
            policy,
            Some(&Admission::new()),
            None
        )
        .await,
        Err(WarmPoolError::Unavailable)
    ));
    assert_eq!(
        store.rows.lock().unwrap()[0].error.as_deref(),
        Some("unavailable")
    );
    store.rows.lock().unwrap()[0].error = None;

    for observed in [1, 2] {
        let mut status = ready.clone();
        status.ready_replicas = observed;
        *planner.inventory.lock().unwrap() = vec![status.clone()];
        // Promotion requires the complete target, even when startup is enabled.
        assert_eq!(
            enabled
                .candidate_inventory(&original)
                .await
                .unwrap()
                .is_some(),
            observed == 2
        );
        assert_eq!(
            original.view(Some(&status), true).status,
            if observed == 2 { "ready" } else { "pending" }
        );
    }
    assert_eq!(store.rows.lock().unwrap()[0].binding, original.binding);
}

#[test]
fn empty_capacity_requires_an_unambiguous_exact_observation() {
    let (_, provider) = fixture();
    let mut target = target();
    target.binding = Some(provider.binding());
    target.applied_revision = target.revision;
    let zero = NativePoolStatus {
        binding: provider.binding(),
        desired_replicas: target.target_replicas,
        ready_replicas: 0,
        observed: true,
    };
    assert_eq!(
        observed_capacity(&target, std::slice::from_ref(&zero)),
        Some(0)
    );
    assert_eq!(
        observed_capacity(&target, &[zero.clone(), zero.clone()]),
        None
    );
    for changed in 0..3 {
        let mut status = zero.clone();
        match changed {
            0 => status.binding.uid = "replacement".into(),
            1 => status.desired_replicas += 1,
            _ => status.observed = false,
        }
        assert_eq!(observed_capacity(&target, &[status]), None);
    }
    target.applied_revision = 0;
    assert_eq!(
        observed_capacity(&target, std::slice::from_ref(&zero)),
        None
    );
    target.applied_revision = target.revision;
    target.error = Some("unavailable".into());
    assert_eq!(observed_capacity(&target, &[zero]), None);
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
        inventory: Arc::new(Mutex::new(inventory)),
        ..Default::default()
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

#[tokio::test(start_paused = true)]
async fn startup_pool_drift_requires_exact_target_and_preserves_healthy_capacity_gate() {
    for allow_empty in [false, true] {
        let store = Arc::new(resize::Targets::default());
        let planner = resize::Planner::default();
        let image = resize::pinned("agent", 'a');
        let pool = resize::deployment(store.clone(), planner.clone(), &image, None);
        pool.ensure_default().await.unwrap();
        let original = {
            let mut rows = store.rows.lock().unwrap();
            let row = rows.first_mut().unwrap();
            row.target_replicas = 2;
            row.applied_revision = row.revision;
            row.binding = Some(resize::Planner::binding(&row.plan));
            row.clone()
        };
        let mut observation = NativePoolStatus {
            binding: original.binding.clone().unwrap(),
            desired_replicas: 2,
            ready_replicas: 0,
            observed: false,
        };
        planner.inventory.lock().unwrap().push(observation.clone());
        planner.inventory_drift.store(true, Ordering::SeqCst);
        let policy = StartupPoolPolicy {
            allow_empty,
            target_fencing: true,
        };
        let await_pool = || {
            await_ready_with_fencing(
                &*store,
                &planner,
                &original.plan,
                tokio::time::Instant::now() + Duration::from_secs(3),
                policy,
                None,
                None,
            )
        };

        // Drift on a historical pool is no proof for the current target.
        assert!(matches!(
            await_pool().await,
            Err(WarmPoolError::Unavailable)
        ));
        observation.ready_replicas = 1;
        observation.observed = true;
        planner.inventory.lock().unwrap()[0] = observation;
        assert!(matches!(await_pool().await, Ok(WarmPoolStartup::Ready)));

        planner.bound_drift.store(true, Ordering::SeqCst);
        planner.superseded_target.store(true, Ordering::SeqCst);
        assert!(matches!(await_pool().await, Err(WarmPoolError::Conflict)));
        planner.superseded_target.store(false, Ordering::SeqCst);
        assert!(matches!(
            await_pool().await,
            Ok(WarmPoolStartup::ConfigurationDrift)
        ));
        assert_eq!(
            store.find(&original.id).await.unwrap().unwrap().binding,
            original.binding
        );
        assert_eq!(
            store
                .find(&original.id)
                .await
                .unwrap()
                .unwrap()
                .target_replicas,
            2
        );

        planner.retiring.store(true, Ordering::SeqCst);
        assert!(matches!(await_pool().await, Err(WarmPoolError::Conflict)));
    }
}

pub(crate) async fn assert_startup_pool_with_historical_template_absent(
    provider: &dyn NativePoolControl,
    plan: NativePoolConfiguration,
    active: NativePoolStatus,
) {
    let store = resize::Targets::default();
    let mut target = default_target(
        plan.clone(),
        ConfigureWarmPoolBody {
            cpu_millis: plan.spec.cpu_millis,
            memory_mib: plan.spec.memory_mib,
            target_replicas: active.desired_replicas,
        },
    );
    target.binding = Some(active.binding);
    target.applied_revision = target.revision;
    store.rows.lock().unwrap().push(target);
    for allow_empty in [false, true] {
        assert!(matches!(
            await_ready_with_fencing(
                &store,
                provider,
                &plan,
                tokio::time::Instant::now() + Duration::from_secs(3),
                StartupPoolPolicy {
                    allow_empty,
                    target_fencing: true
                },
                None,
                None,
            )
            .await,
            Ok(WarmPoolStartup::Ready)
        ));
    }
}
