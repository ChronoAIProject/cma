use super::*;
use std::sync::Arc;

struct DelayedPlatform(Duration);

#[async_trait::async_trait]
impl crate::sandbox::reconciliation::platform::PlatformSpendSweep for DelayedPlatform {
    async fn evaluate_platform_spend(
        &self,
        _now: u64,
    ) -> Result<Option<bool>, layer2_core::budget::BudgetError> {
        tokio::time::sleep(self.0).await;
        Ok(None)
    }
}

fn background() -> crate::background::BackgroundWork {
    crate::background::BackgroundWork::new(
        crate::background::BackgroundBudget::new(),
        "sandbox_reconciliation",
        Duration::from_secs(30),
        Duration::ZERO,
    )
}

#[tokio::test(start_paused = true)]
async fn reserved_opportunity_includes_platform_and_progress_lock_awaits() {
    for (enabled, lock_seconds, prelude_seconds, expected) in [
        (false, 0, 6, 26),
        (true, 0, 0, 25),
        (true, 0, 6, 25),
        (true, 6, 6, 25),
    ] {
        let (store, reconciler) = setup(2, Duration::ZERO).await;
        let reconciler = reconciler
            .with_runtime_loss_bounded_proof(enabled)
            .with_platform_spend(Arc::new(DelayedPlatform(Duration::from_secs(
                prelude_seconds,
            ))));
        reserve(&reconciler, 0, "deadline").await;
        bindings(&reconciler, 2).await;
        store.fail("get", "sbx_000", Failure::Delay(Duration::from_secs(26)));
        let work = background();
        let guard = reconciler.progress.lock().await;
        let release = async {
            tokio::time::sleep(Duration::from_secs(lock_seconds)).await;
            drop(guard);
        };
        let started = Instant::now();
        let (_, result) = tokio::join!(
            release,
            work.run_resumable(|| reconciler.run_due(SystemTime::now()))
        );
        assert_eq!(started.elapsed(), Duration::from_secs(expected));
        assert_eq!(result.unwrap(), Some(false));
        assert!(reconciler.binding_holds.contains("sbx_000"));
        assert_eq!(store.keys("get"), ["sbx_000"]);
    }
}

#[tokio::test(start_paused = true)]
async fn exhausted_prelude_keeps_five_seconds_and_never_dispatches_binding_work() {
    for (lock_seconds, prelude_seconds) in [(25, 0), (0, 25), (26, 0), (0, 26)] {
        let (store, reconciler) = setup(2, Duration::ZERO).await;
        let reconciler = reconciler
            .with_runtime_loss_bounded_proof(true)
            .with_platform_spend(Arc::new(DelayedPlatform(Duration::from_secs(
                prelude_seconds,
            ))));
        reserve(&reconciler, 0, "deadline").await;
        bindings(&reconciler, 2).await;
        let work = background();
        let guard = reconciler.progress.lock().await;
        let release = async {
            tokio::time::sleep(Duration::from_secs(lock_seconds)).await;
            drop(guard);
        };
        let observed = async {
            let started = Instant::now();
            let result = work
                .run_resumable(|| reconciler.run_due(SystemTime::now()))
                .await;
            (started.elapsed(), result)
        };
        let (_, (elapsed, result)) = tokio::join!(release, observed);
        assert_eq!(elapsed, Duration::from_secs(25));
        assert!(
            matches!(result, Err(SandboxStoreError::Unavailable(message))
            if message == "sandbox reconciliation prelude deadline exceeded")
        );
        assert!(store.keys("get").is_empty());
        assert!(reconciler.binding_holds.contains("sbx_000"));
        assert!(reconciler
            .progress
            .lock()
            .await
            .as_ref()
            .unwrap()
            .sweep
            .binding_cursor
            .is_none());
        // Both the serial guard and shared background permit are released.
        assert_eq!(
            work.run_resumable(|| async { Ok(true) }).await.unwrap(),
            Some(true)
        );
        let reconciler = reconciler.with_platform_spend(Arc::new(DelayedPlatform(Duration::ZERO)));
        assert_eq!(
            work.run_resumable(|| reconciler.run_due(SystemTime::now()))
                .await
                .unwrap(),
            Some(false)
        );
        assert_eq!(store.keys("get"), ["sbx_000"]);
        assert!(!reconciler.binding_holds.contains("sbx_000"));
    }
}

#[tokio::test(start_paused = true)]
async fn a_binding_can_finish_within_the_budget_remaining_after_the_prelude() {
    let (store, reconciler) = setup(2, Duration::ZERO).await;
    let reconciler = reconciler
        .with_runtime_loss_bounded_proof(true)
        .with_platform_spend(Arc::new(DelayedPlatform(Duration::from_secs(6))));
    reserve(&reconciler, 0, "deadline").await;
    bindings(&reconciler, 2).await;
    store.fail("get", "sbx_000", Failure::Delay(Duration::from_secs(18)));
    let started = Instant::now();
    assert_eq!(
        background()
            .run_resumable(|| reconciler.run_due(SystemTime::now()))
            .await
            .unwrap(),
        Some(false)
    );
    assert_eq!(started.elapsed(), Duration::from_secs(24));
    assert!(!reconciler.binding_holds.contains("sbx_000"));
    assert_eq!(store.keys("get"), ["sbx_000"]);
}

#[tokio::test(start_paused = true)]
async fn cancellation_after_prelude_keeps_protection_and_releases_the_opportunity() {
    let (store, reconciler) = setup(2, Duration::ZERO).await;
    let reconciler = reconciler
        .with_runtime_loss_bounded_proof(true)
        .with_platform_spend(Arc::new(DelayedPlatform(Duration::from_secs(6))));
    reserve(&reconciler, 0, "deadline").await;
    bindings(&reconciler, 2).await;
    store.fail("get", "sbx_000", Failure::Delay(Duration::from_secs(26)));
    let work = background();
    let started = Instant::now();
    assert!(tokio::time::timeout(
        Duration::from_secs(7),
        work.run_resumable(|| reconciler.run_due(SystemTime::now()))
    )
    .await
    .is_err());
    assert_eq!(started.elapsed(), Duration::from_secs(7));
    assert!(reconciler.binding_holds.contains("sbx_000"));
    assert_eq!(
        work.run_resumable(|| reconciler.run_due(SystemTime::now()))
            .await
            .unwrap(),
        Some(true)
    );
    assert_eq!(store.keys("get"), ["sbx_000", "sbx_001"]);
    assert_eq!(store.keys("pause"), ["sbx_001"]);
}
