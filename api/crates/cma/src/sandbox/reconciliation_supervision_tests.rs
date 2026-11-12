use super::*;

async fn settle() {
    for _ in 0..30 {
        tokio::task::yield_now().await;
    }
}

fn age(activity: &crate::worker_supervision::Activity) -> u64 {
    serde_json::to_value(activity.snapshot()).unwrap()["reconciliation_age_ms"]
        .as_u64()
        .unwrap()
}

#[tokio::test(start_paused = true)]
async fn runtime_reconciliation_retains_long_batch_through_panic_and_drains_it_once() {
    let (store, reconciler) = setup(8, Duration::from_secs(2)).await;
    let activity = crate::worker_supervision::Workers::new().get("sandbox_reconciliation");
    let (stop, signal) = tokio::sync::watch::channel(false);
    let scheduler = reconciler.spawn_supervised(signal, activity.clone());
    settle().await;
    assert_eq!(store.keys("authority"), ["start"]);
    assert!(activity.snapshot().available());
    for _ in 0..7 {
        tokio::time::advance(Duration::from_secs(1)).await;
        settle().await;
        assert_eq!(activity.snapshot().state, "running");
    }
    assert!(age(&activity) >= 7_000);
    crate::worker_supervision::panic_next_poll(&activity);
    tokio::time::advance(Duration::from_secs(1)).await;
    settle().await;
    assert_eq!(activity.snapshot().state, "backoff");
    assert!(!activity.snapshot().available());
    tokio::time::advance(Duration::from_secs(1)).await;
    settle().await;
    assert_eq!(activity.snapshot().state, "running");
    // The phase guard belongs to the original batch, not a replacement child.
    assert!(age(&activity) >= 9_000);
    assert_eq!(
        store
            .keys("authority")
            .iter()
            .filter(|key| *key == "start")
            .count(),
        1
    );
    let before = Instant::now();
    stop.send(true).unwrap();
    settle().await;
    assert_eq!(activity.snapshot().state, "draining");
    assert!(!scheduler.is_finished());
    scheduler.await.unwrap();
    assert!(Instant::now() > before);
    assert_eq!(activity.snapshot().state, "stopped");
    assert_eq!(
        store
            .keys("authority")
            .iter()
            .filter(|key| *key == "start")
            .count(),
        1
    );
    assert_eq!(
        store.keys("authority"),
        ["start", "sbx_001", "sbx_003", "sbx_005"]
    );
}
