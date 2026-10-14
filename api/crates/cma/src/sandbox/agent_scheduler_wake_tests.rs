use super::*;

#[tokio::test(start_paused = true)]
async fn admission_wake_discovers_before_next_tick_without_extra_reconciliation() {
    let probe = Arc::new(Probe::default());
    let scheduler = probe.scheduler(2);
    let admitted = scheduler.admitted.clone();
    let (shutdown, signal) = watch::channel(false);
    let scheduler = scheduler.spawn(signal);
    settle().await;
    let before = tokio::time::Instant::now();

    let id = probe.put("agt_new", Behavior::Complete);
    admitted.notify_one();
    settle().await;
    assert!(probe.completed.lock().unwrap().contains(&id));
    assert_eq!(tokio::time::Instant::now(), before);
    assert_eq!(probe.polls.load(Ordering::SeqCst), 2);
    assert_eq!(probe.reconciles.load(Ordering::SeqCst), 1);
    shutdown.send(true).unwrap();
    scheduler.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn admission_without_a_wake_is_recovered_by_periodic_discovery() {
    let probe = Arc::new(Probe::default());
    let (shutdown, signal) = watch::channel(false);
    let scheduler = probe.scheduler(2).spawn(signal);
    settle().await;
    let id = probe.put("agt_missed", Behavior::Complete);
    settle().await;
    assert_eq!(probe.calls(&id), 0);
    tokio::time::advance(SWEEP_INTERVAL).await;
    settle().await;
    assert!(probe.completed.lock().unwrap().contains(&id));
    shutdown.send(true).unwrap();
    scheduler.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn wake_storm_retains_one_discovery_through_full_capacity() {
    let probe = Arc::new(Probe::default());
    let release = Arc::new(Notify::new());
    let running = probe.put("agt_running", Behavior::Wait(release.clone()));
    let scheduler = probe.scheduler(1);
    let admitted = scheduler.admitted.clone();
    let (shutdown, signal) = watch::channel(false);
    let scheduler = scheduler.spawn(signal);
    settle().await;
    let id = probe.put("agt_new", Behavior::Complete);
    for _ in 0..1_000 {
        admitted.notify_one();
    }
    settle().await;
    assert_eq!(probe.polls.load(Ordering::SeqCst), 1);
    assert_eq!(probe.calls(&id), 0);
    assert_eq!(probe.active.load(Ordering::SeqCst), 1);
    let before = tokio::time::Instant::now();
    release.notify_one();
    settle().await;
    assert_eq!(tokio::time::Instant::now(), before);
    assert!(probe.completed.lock().unwrap().contains(&id));
    assert_eq!(probe.calls(&running), 1);
    assert_eq!(probe.polls.load(Ordering::SeqCst), 2);
    assert_eq!(probe.peak.load(Ordering::SeqCst), 1);
    shutdown.send(true).unwrap();
    scheduler.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn admission_during_stale_discovery_preserves_wake_for_a_fresh_query() {
    let probe = Arc::new(Probe::default());
    let release = Arc::new(Notify::new());
    *probe.poll_release.lock().unwrap() = Some(release.clone());
    let scheduler = probe.scheduler(2);
    let admitted = scheduler.admitted.clone();
    let (shutdown, signal) = watch::channel(false);
    let scheduler = scheduler.spawn(signal);
    settle().await;
    // The first discovery captured an empty snapshot before this admission.
    let id = probe.put("agt_new", Behavior::Complete);
    admitted.notify_one();
    settle().await;
    assert_eq!(probe.polls.load(Ordering::SeqCst), 1);
    assert_eq!(probe.calls(&id), 0);
    let before = tokio::time::Instant::now();
    release.notify_one();
    settle().await;
    assert_eq!(tokio::time::Instant::now(), before);
    assert_eq!(probe.polls.load(Ordering::SeqCst), 2);
    assert!(probe.completed.lock().unwrap().contains(&id));
    shutdown.send(true).unwrap();
    scheduler.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn retained_admission_wake_does_not_start_work_after_shutdown() {
    let probe = Arc::new(Probe::default());
    let release = Arc::new(Notify::new());
    probe.put("agt_running", Behavior::Wait(release.clone()));
    let scheduler = probe.scheduler(1);
    let admitted = scheduler.admitted.clone();
    let (shutdown, signal) = watch::channel(false);
    let scheduler = scheduler.spawn(signal);
    settle().await;
    let id = probe.put("agt_queued", Behavior::Complete);
    admitted.notify_one();
    shutdown.send(true).unwrap();
    settle().await;
    release.notify_one();
    scheduler.await.unwrap();
    assert_eq!(probe.calls(&id), 0);
    assert_eq!(probe.polls.load(Ordering::SeqCst), 1);
}
