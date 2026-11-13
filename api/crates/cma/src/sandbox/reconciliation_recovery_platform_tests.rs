use super::*;

struct Platform(Mutex<Option<bool>>);

#[async_trait::async_trait]
impl crate::sandbox::reconciliation::platform::PlatformSpendSweep for Platform {
    async fn evaluate_platform_spend(
        &self,
        _: u64,
    ) -> Result<Option<bool>, layer2_core::budget::BudgetError> {
        Ok(*self.0.lock().unwrap())
    }
}

fn now() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_788_566_400)
}

#[tokio::test(start_paused = true)]
async fn platform_spend_stop_defers_recovery_discovery_until_lift() {
    let (store, reconciler, verifier) = recovery_setup(1, None).await;
    let platform = Arc::new(Platform(Mutex::new(Some(true))));
    let reconciler = reconciler.with_platform_spend(platform.clone());
    let id = sandbox(0).id;
    let before = store.inner.get_sandbox(&id).await.unwrap();
    let at = now();
    let mut sweeps = Vec::new();
    for _ in 0..6 {
        sweeps.push(reconciler.run_due(at).await.unwrap());
    }
    assert_eq!(
        *verifier.calls.lock().unwrap(),
        [],
        "SpendStop must defer native recovery proofs"
    );
    assert!(store.keys("recovery_snapshot").is_empty());
    assert!(store.keys("recovery_admit").is_empty());
    // Only ordinary sweeps discover authority and reread the binding. There
    // must be no additional discovery or get from the recovery lane.
    assert_eq!(store.keys("authority"), ["start"; 6]);
    assert_eq!(store.keys("get"), vec![id.to_string(); 6]);
    assert_eq!(store.keys("pause"), vec![id.to_string(); 6]);
    assert!(sweeps.iter().all(|complete| *complete));
    assert_eq!(store.inner.get_sandbox(&id).await.unwrap(), before);

    *platform.0.lock().unwrap() = Some(false);
    let lifted_at = at + Duration::from_secs(15);
    // STOP consumed no turns: ordinary work, discovery, ordinary work, proof.
    assert!(reconciler.run_due(lifted_at).await.unwrap());
    assert!(!reconciler.run_due(lifted_at).await.unwrap());
    assert!(verifier.calls.lock().unwrap().is_empty());
    assert!(reconciler.run_due(lifted_at).await.unwrap());
    assert!(!reconciler.run_due(lifted_at).await.unwrap());
    assert_eq!(
        verifier.calls.lock().unwrap().as_slice(),
        std::slice::from_ref(&id)
    );
    assert_eq!(store.keys("authority"), ["start"; 9]);
    assert_eq!(store.keys("get"), vec![id.to_string(); 9]);
    assert_eq!(store.keys("recovery_snapshot"), [id.to_string()]);
    assert_eq!(store.keys("recovery_admit"), [id.to_string()]);
}

#[tokio::test(start_paused = true)]
async fn platform_spend_stop_preserves_pending_recovery_until_lift() {
    for open in [Some(false), None] {
        let (store, reconciler, verifier) = recovery_setup(1, None).await;
        let platform = Arc::new(Platform(Mutex::new(open)));
        let reconciler = reconciler.with_platform_spend(platform.clone());
        let id = sandbox(0).id;
        let before = store.inner.get_sandbox(&id).await.unwrap();
        let at = now();
        assert!(reconciler.run_due(at).await.unwrap());
        assert!(!reconciler.run_due(at).await.unwrap());
        assert_eq!(store.keys("authority"), ["start"; 2]);
        assert!(verifier.calls.lock().unwrap().is_empty());

        *platform.0.lock().unwrap() = Some(true);
        let stopped_at = at + Duration::from_secs(15);
        let mut sweeps = Vec::new();
        for _ in 0..5 {
            sweeps.push(reconciler.run_due(stopped_at).await.unwrap());
        }
        assert_eq!(
            *verifier.calls.lock().unwrap(),
            [],
            "SpendStop must retain a pending candidate without proving it"
        );
        assert!(store.keys("recovery_snapshot").is_empty());
        assert!(store.keys("recovery_admit").is_empty());
        assert_eq!(store.keys("authority"), ["start"; 7]);
        assert_eq!(store.keys("get"), vec![id.to_string(); 6]);
        // Include the initial ordinary sweep's inactivity pause request.
        assert_eq!(store.keys("pause"), vec![id.to_string(); 6]);
        assert!(sweeps.iter().all(|complete| *complete));
        assert_eq!(store.inner.get_sandbox(&id).await.unwrap(), before);

        *platform.0.lock().unwrap() = open;
        let lifted_at = stopped_at + Duration::from_secs(15);
        assert!(reconciler.run_due(lifted_at).await.unwrap());
        assert!(!reconciler.run_due(lifted_at).await.unwrap());
        // The pending ID survives STOP: proof needs no new recovery discovery.
        assert_eq!(
            verifier.calls.lock().unwrap().as_slice(),
            std::slice::from_ref(&id)
        );
        assert_eq!(store.keys("authority"), ["start"; 8]);
        assert_eq!(store.keys("get"), vec![id.to_string(); 8]);
        assert_eq!(store.keys("recovery_snapshot"), [id.to_string()]);
        assert_eq!(store.keys("recovery_admit"), [id.to_string()]);
    }
}
