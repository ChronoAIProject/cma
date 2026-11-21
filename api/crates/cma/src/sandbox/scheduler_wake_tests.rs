use super::*;

async fn settle() {
    for _ in 0..30 {
        tokio::task::yield_now().await;
    }
}

#[tokio::test(start_paused = true)]
async fn admission_wakes_existing_sandbox_scheduler_before_next_tick() {
    let probe = Arc::new(SweepProbe::default());
    let admitted = Arc::new(Notify::new());
    let (shutdown, signal) = watch::channel(false);
    let scheduler = SandboxScheduler::with_interval(probe.clone(), DEFAULT_SWEEP_INTERVAL)
        .with_admission_wake(admitted.clone())
        .spawn(signal);
    settle().await;
    assert_eq!(probe.calls.load(Ordering::SeqCst), 1);
    let before = tokio::time::Instant::now();
    admitted.notify_one();
    settle().await;
    assert_eq!(tokio::time::Instant::now(), before);
    assert_eq!(probe.calls.load(Ordering::SeqCst), 2);
    // A later durable admission with a missed local hint is still discovered.
    tokio::time::advance(DEFAULT_SWEEP_INTERVAL).await;
    settle().await;
    assert_eq!(probe.calls.load(Ordering::SeqCst), 3);
    shutdown.send(true).unwrap();
    scheduler.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn admission_wake_storm_preserves_sandbox_capacity_and_one_pending_sweep() {
    let probe = blocking_probe(true);
    let admitted = Arc::new(Notify::new());
    let (shutdown, signal) = watch::channel(false);
    let mut scheduler = SandboxScheduler::with_interval(probe.clone(), DEFAULT_SWEEP_INTERVAL)
        .with_admission_wake(admitted.clone());
    scheduler.concurrent_sweeps = SANDBOX_CONCURRENT_SWEEPS;
    let scheduler = scheduler.spawn(signal);
    settle().await;
    for _ in 1..SANDBOX_CONCURRENT_SWEEPS {
        admitted.notify_one();
        settle().await;
    }
    assert_eq!(
        probe.active.load(Ordering::SeqCst),
        SANDBOX_CONCURRENT_SWEEPS
    );
    for _ in 0..1_000 {
        admitted.notify_one();
    }
    settle().await;
    assert_eq!(
        probe.calls.load(Ordering::SeqCst),
        SANDBOX_CONCURRENT_SWEEPS
    );
    let before = tokio::time::Instant::now();
    probe.release.notify_one();
    settle().await;
    assert_eq!(tokio::time::Instant::now(), before);
    assert_eq!(
        probe.calls.load(Ordering::SeqCst),
        SANDBOX_CONCURRENT_SWEEPS + 1
    );
    assert_eq!(
        probe.active.load(Ordering::SeqCst),
        SANDBOX_CONCURRENT_SWEEPS
    );
    // Another retained wake must not bypass a requested shutdown.
    admitted.notify_one();
    shutdown.send(true).unwrap();
    settle().await;
    probe.release.notify_waiters();
    scheduler.await.unwrap();
    assert_eq!(
        probe.calls.load(Ordering::SeqCst),
        SANDBOX_CONCURRENT_SWEEPS + 1
    );
    assert_eq!(probe.active.load(Ordering::SeqCst), 0);
}
