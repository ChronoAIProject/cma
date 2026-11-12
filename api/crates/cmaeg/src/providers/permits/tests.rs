use super::*;
fn gate() -> Arc<Gate> {
    Arc::new(Gate::new("fixture".into(), 1))
}
#[tokio::test]
async fn saturation_timeout_cancel_and_release_preserve_capacity() {
    let gate = gate();
    let held = acquire(&gate).await.unwrap();
    assert_eq!(gate.state.lock().unwrap().held, 1);
    assert!(matches!(gate.try_acquire(), Err(ProviderError::Capacity)));
    {
        let pending = acquire(&gate);
        tokio::pin!(pending);
        assert!(futures_util::poll!(&mut pending).is_pending());
        assert_eq!(gate.state.lock().unwrap().waiting.len(), 1);
    }
    assert!(gate.state.lock().unwrap().waiting.is_empty());
    assert!(matches!(
        acquire(&gate).await,
        Err(ProviderError::Busy { .. })
    ));
    assert!(gate.state.lock().unwrap().waiting.is_empty());
    drop(held);
    assert_eq!(gate.semaphore.available_permits(), 1);
    assert_eq!(gate.state.lock().unwrap().held, 0);
    drop(acquire(&gate).await.unwrap());
}
#[tokio::test]
async fn stream_task_owns_permit_until_disconnect_or_panic() {
    let gate = gate();
    let permit = acquire(&gate).await.unwrap();
    let (sender, receiver) = tokio::sync::mpsc::channel::<()>(1);
    let task = tokio::spawn(async move {
        let _permit = permit;
        sender.closed().await;
    });
    assert!(matches!(gate.try_acquire(), Err(ProviderError::Capacity)));
    drop(receiver);
    task.await.unwrap();
    let permit = acquire(&gate).await.unwrap();
    let task = tokio::spawn(async move {
        let _permit = permit;
        panic!("fixture");
    });
    assert!(task.await.unwrap_err().is_panic());
    assert_eq!(gate.state.lock().unwrap().held, 0);
    assert_eq!(gate.semaphore.available_permits(), 1);
}
#[tokio::test]
async fn metrics_count_each_attempt_once_and_clear_oldest_waiter() {
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    let _local = metrics::set_default_local_recorder(&recorder);
    let gate = gate();
    let permit = gate.try_acquire().unwrap();
    assert!(matches!(gate.try_acquire(), Err(ProviderError::Capacity)));
    {
        let pending = acquire(&gate);
        tokio::pin!(pending);
        assert!(futures_util::poll!(&mut pending).is_pending());
        let text = handle.render();
        assert!(text.contains("cmaeg_provider_permit_waiters{provider=\"fixture\"} 1"));
        assert!(!text.contains(
            "cmaeg_provider_permit_oldest_wait_timestamp_seconds{provider=\"fixture\"} 0"
        ));
    }
    drop(permit);
    let text = handle.render();
    for (name, value) in [
        ("permits_in_use", 0),
        ("permits_available", 1),
        ("permit_waiters", 0),
        ("permit_oldest_wait_timestamp_seconds", 0),
    ] {
        assert!(
            text.contains(&format!(
                "cmaeg_provider_{name}{{provider=\"fixture\"}} {value}"
            )),
            "{text}"
        );
    }
    for outcome in ["acquired", "full", "cancelled"] {
        assert!(
            text.lines().any(
                |line| line.starts_with("cmaeg_provider_permit_acquisitions_total{")
                    && line.contains(&format!("outcome=\"{outcome}\""))
                    && line.ends_with(" 1")
            ),
            "{text}"
        );
    }
}
#[tokio::test]
async fn closed_gate_records_failure_without_leaking_waiters() {
    let gate = gate();
    gate.semaphore.close();
    assert!(matches!(
        acquire(&gate).await,
        Err(ProviderError::Unavailable(_))
    ));
    assert!(gate.state.lock().unwrap().waiting.is_empty());
}
