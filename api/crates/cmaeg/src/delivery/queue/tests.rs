use super::*;

#[tokio::test]
async fn saturation_cancel_and_receiver_drop_release_all_residences() {
    let inventory = Arc::new(Inventory::default());
    let (sender, mut receiver) = channel(1, inventory.clone());
    sender.send("first").await.unwrap();
    assert_eq!(inventory.0.lock().unwrap().queued.len(), 1);
    let blocked = sender.send("second");
    tokio::pin!(blocked);
    assert!(futures_util::poll!(&mut blocked).is_pending());
    // A send waiting on capacity is not a queued item and can be cancelled.
    assert_eq!(inventory.0.lock().unwrap().queued.len(), 1);
    assert_eq!(receiver.recv().await, Some("first"));
    assert!(inventory.0.lock().unwrap().queued.is_empty());
    blocked.await.unwrap();
    assert_eq!(inventory.0.lock().unwrap().queued.len(), 1);
    drop(receiver);
    assert!(inventory.0.lock().unwrap().queued.is_empty());
    assert_eq!(sender.send("closed").await.unwrap_err().0, "closed");
}

#[tokio::test]
async fn worker_panic_releases_queued_entries_and_oldest_age_is_live() {
    let inventory = Arc::new(Inventory::default());
    let (sender, receiver) = channel(2, inventory.clone());
    sender.send(1).await.unwrap();
    sender.send(2).await.unwrap();
    assert_eq!(inventory.0.lock().unwrap().queued.len(), 2);
    let task = tokio::spawn(async move {
        let _receiver = receiver;
        panic!("fixture");
    });
    assert!(task.await.unwrap_err().is_panic());
    assert!(inventory.0.lock().unwrap().queued.is_empty());
}

#[tokio::test]
async fn cancelled_wait_does_not_create_a_phantom_entry() {
    let inventory = Arc::new(Inventory::default());
    let (sender, receiver) = channel(1, inventory.clone());
    sender.send(1).await.unwrap();
    {
        let blocked = sender.send(2);
        tokio::pin!(blocked);
        assert!(futures_util::poll!(&mut blocked).is_pending());
    }
    assert_eq!(inventory.0.lock().unwrap().queued.len(), 1);
    drop(receiver);
    assert!(inventory.0.lock().unwrap().queued.is_empty());
}

#[tokio::test]
async fn oldest_continues_aging_while_no_command_is_consumed() {
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    let _metrics = metrics::set_default_local_recorder(&recorder);
    let inventory = Arc::new(Inventory::default());
    let _monitor = inventory.monitor();
    let (sender, receiver) = channel(1, inventory);
    sender.send(1).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    let text = handle.render();
    let age: f64 = text
        .lines()
        .find(|line| line.starts_with("cmaeg_delivery_queue_oldest_seconds{"))
        .unwrap()
        .split_whitespace()
        .last()
        .unwrap()
        .parse()
        .unwrap();
    assert!(age >= 1., "oldest stalled while worker was blocked: {text}");
    drop(receiver);
    let text = handle.render();
    assert!(text.contains("cmaeg_delivery_queue_depth{stage=\"worker\"} 0"));
    assert!(text.contains("cmaeg_delivery_queue_oldest_seconds{stage=\"worker\"} 0"));
}
