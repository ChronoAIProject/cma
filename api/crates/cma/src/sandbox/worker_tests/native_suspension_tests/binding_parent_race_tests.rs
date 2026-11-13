//! The parent changes inside the current reserved adoption/status attempt.
use super::*;
use layer3_adapter::HomeVolumeStore;

#[tokio::test(start_paused = true)]
async fn due_reserved_status_proof_preserves_concurrent_parent_revision_and_children() {
    let mut unprotected = Vec::new();
    for boundary in [false, true] {
        let (fixture, active, now) = active_fixture().await;
        let agent =
            lifecycle_recovery_tests::prepare_agent(&fixture.harness, &active, now, 1).await;
        let homes = fixture.homes.inventory().await.unwrap();
        let calls = fixture.backend.calls.snapshot();
        let actions = fixture.harness.actions.snapshot();
        let permits = Arc::new(Semaphore::new(1));
        let fired = Arc::new(AtomicBool::new(false));
        let observed = fired.clone();
        let store = fixture.harness.store.clone();
        let mut changed = active.clone();
        changed.revision += 1;
        changed.name = "concurrent parent revision".into();
        let newer = changed.clone();
        let expected_revision = active.revision;
        let hook: StatusHook = Arc::new(move |point| {
            let store = store.clone();
            let newer = newer.clone();
            let observed = observed.clone();
            Box::pin(async move {
                if point == boundary && !observed.swap(true, Ordering::SeqCst) {
                    store.save_sandbox(expected_revision, newer).await.unwrap();
                }
            })
        });
        let backend = SlowBackend {
            inner: fixture.backend.clone(),
            adoption: Duration::from_secs(3),
            status: Duration::from_secs(3),
            permits: permits.clone(),
            status_hook: Some(hook),
        };
        let mut config = CodexSandboxSpec::default();
        config.deployment.image.uri = runtime_image();
        let adapter = Arc::new(
            CodexAdapter::with_backend(backend, config).with_home_volumes(fixture.homes.clone()),
        );
        let reconciler = SandboxReconciler::new(
            fixture.harness.store.clone(),
            adapter,
            Arc::new(fixture.harness.policy.clone()),
            "test".into(),
        );
        // The first five-second attempt expires before either fault hook.
        assert!(reconciler.run_due(now).await.unwrap());
        assert!(!fired.load(Ordering::SeqCst));
        assert!(reconciler.binding_is_held(&active.id));
        tokio::time::advance(Duration::from_secs(30)).await;
        assert!(!reconciler.run_due(now).await.unwrap());
        assert!(!fired.load(Ordering::SeqCst));
        let started = tokio::time::Instant::now();
        assert!(!reconciler.run_due(now).await.unwrap());
        assert_eq!(started.elapsed(), Duration::from_secs(6));
        assert!(fired.load(Ordering::SeqCst));
        assert_eq!(permits.available_permits(), 1);
        assert_eq!(fixture.harness.stored(&active.id).await, changed);
        let held = reconciler.binding_is_held(&active.id);
        // Finish the retained cleanup tail while the raced binding is held.
        assert!(reconciler.run_due(now).await.unwrap());
        assert_eq!(fixture.homes.inventory().await.unwrap(), homes);
        assert_eq!(fixture.backend.calls.snapshot(), calls);
        assert_eq!(fixture.harness.actions.snapshot(), actions);
        assert_eq!(
            fixture.harness.store.get_agent(&agent.id).await.unwrap(),
            Some(agent)
        );
        assert_eq!(fixture.harness.stored(&active.id).await, changed);
        if !held || !reconciler.binding_is_held(&active.id) {
            unprotected.push(boundary);
        }
    }
    assert!(
        unprotected.is_empty(),
        "parent revision raced status but protection was cleared (after response): {unprotected:?}"
    );
}
