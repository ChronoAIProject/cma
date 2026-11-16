use super::*;

#[tokio::test(start_paused = true)]
async fn cancel_wake_starts_well_before_scan_tick_and_coalesces_bursts() {
    let probe = Arc::new(Probe::default());
    let wake = Arc::new(Notify::new());
    let (shutdown, signal) = watch::channel(false);
    let scheduler = probe
        .scheduler(2)
        .with_discovery_wake(Some(wake.clone()))
        .spawn(signal);
    settle().await;
    tokio::time::advance(Duration::from_millis(1)).await;
    let first = probe.put("first", Behavior::Complete);
    let committed = Instant::now();
    for _ in 0..1_000 {
        wake.notify_one();
    }
    settle().await;
    assert_eq!(probe.count(&first), 1);
    assert!(committed.elapsed() < Duration::from_millis(25));
    assert_eq!(probe.discoveries.load(Ordering::SeqCst), 2);
    let next = probe.put("next", Behavior::Complete);
    for _ in 0..1_000 {
        wake.notify_one();
    }
    settle().await;
    tokio::time::advance(Duration::from_millis(49)).await;
    settle().await;
    assert_eq!(probe.count(&next), 0);
    tokio::time::advance(Duration::from_millis(1)).await;
    settle().await;
    assert_eq!(probe.count(&next), 1);
    assert_eq!(probe.discoveries.load(Ordering::SeqCst), 3);
    shutdown.send(true).unwrap();
    scheduler.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn wake_survives_full_slots_without_polling_or_duplicate_execution() {
    let probe = Arc::new(Probe::default());
    let release = Arc::new(Notify::new());
    let slow = probe.put("slow", Behavior::Wait(release.clone()));
    let wake = Arc::new(Notify::new());
    let (shutdown, signal) = watch::channel(false);
    let scheduler = probe
        .scheduler(1)
        .with_discovery_wake(Some(wake.clone()))
        .spawn(signal);
    settle().await;
    let cancel = probe.put("cancel", Behavior::Complete);
    for _ in 0..1_000 {
        wake.notify_one();
    }
    settle().await;
    assert_eq!(probe.discoveries.load(Ordering::SeqCst), 1);
    assert_eq!(probe.count(&cancel), 0);
    release.notify_one();
    settle().await;
    assert_eq!(probe.count(&cancel), 1);
    assert_eq!(probe.count(&slow), 1);
    assert_eq!(probe.peak.load(Ordering::SeqCst), 1);
    shutdown.send(true).unwrap();
    scheduler.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn failed_wake_discovery_and_lost_notifications_recover_on_periodic_tick() {
    let probe = Arc::new(Probe::default());
    let wake = Arc::new(Notify::new());
    let (shutdown, signal) = watch::channel(false);
    let scheduler = probe
        .scheduler(2)
        .with_discovery_wake(Some(wake.clone()))
        .spawn(signal);
    settle().await;
    let cancel = probe.put("cancel", Behavior::Complete);
    probe.fail_poll.store(true, Ordering::SeqCst);
    wake.notify_one();
    settle().await;
    assert_eq!(probe.count(&cancel), 0);
    tokio::time::advance(POLL_INTERVAL).await;
    settle().await;
    assert_eq!(probe.count(&cancel), 1);
    let lost = probe.put("lost", Behavior::Complete);
    tokio::time::advance(POLL_INTERVAL).await;
    settle().await;
    assert_eq!(probe.count(&lost), 1);
    shutdown.send(true).unwrap();
    scheduler.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn shutdown_does_not_start_pending_wake_work() {
    let probe = Arc::new(Probe::default());
    let wake = Arc::new(Notify::new());
    let (shutdown, signal) = watch::channel(false);
    let scheduler = probe
        .scheduler(2)
        .with_discovery_wake(Some(wake.clone()))
        .spawn(signal);
    settle().await;
    let cancel = probe.put("cancel", Behavior::Complete);
    shutdown.send(true).unwrap();
    wake.notify_one();
    scheduler.await.unwrap();
    assert_eq!(probe.count(&cancel), 0);
    assert_eq!(probe.discoveries.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn real_runtime_wake_discovers_without_waiting_for_scan_tick() {
    let probe = Arc::new(Probe::default());
    let wake = Arc::new(Notify::new());
    let (shutdown, signal) = watch::channel(false);
    let scheduler = probe
        .scheduler(2)
        .with_discovery_wake(Some(wake.clone()))
        .spawn(signal);
    // Observe the actual initial scan before introducing newly committed work.
    tokio::time::timeout(Duration::from_secs(2), async {
        while probe.discoveries.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    settle().await;
    let cancel = probe.put("cancel", Behavior::Complete);
    let committed = Instant::now();
    wake.notify_one();
    tokio::time::timeout(Duration::from_millis(100), async {
        while probe.count(&cancel) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("wake must discover well before the 250ms tick");
    assert!(committed.elapsed() < Duration::from_millis(100));
    shutdown.send(true).unwrap();
    scheduler.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn in_flight_discovery_retains_one_pending_wake_without_parallel_scan() {
    let probe = Arc::new(Probe::default());
    let release = Arc::new(Notify::new());
    *probe.discovery_gate.lock().unwrap() = Some(release.clone());
    let wake = Arc::new(Notify::new());
    let (shutdown, signal) = watch::channel(false);
    let scheduler = probe
        .scheduler(2)
        .with_discovery_wake(Some(wake.clone()))
        .spawn(signal);
    settle().await;
    let cancel = probe.put("cancel", Behavior::Complete);
    for _ in 0..1_000 {
        wake.notify_one();
    }
    settle().await;
    assert_eq!(probe.discoveries.load(Ordering::SeqCst), 1);
    assert_eq!(probe.count(&cancel), 0);
    release.notify_one();
    settle().await;
    assert_eq!(probe.discoveries.load(Ordering::SeqCst), 2);
    assert_eq!(probe.count(&cancel), 1);
    shutdown.send(true).unwrap();
    scheduler.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn closed_shutdown_channel_wins_over_pending_wake() {
    let probe = Arc::new(Probe::default());
    let wake = Arc::new(Notify::new());
    let (shutdown, signal) = watch::channel(false);
    let scheduler = probe
        .scheduler(2)
        .with_discovery_wake(Some(wake.clone()))
        .spawn(signal);
    settle().await;
    let cancel = probe.put("cancel", Behavior::Complete);
    drop(shutdown);
    wake.notify_one();
    scheduler.await.unwrap();
    assert_eq!(probe.count(&cancel), 0);
}
