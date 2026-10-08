use super::*;

fn record(index: usize) -> RecordId {
    RecordId::new(format!("00000000-0000-4000-8000-{index:012x}")).unwrap()
}
#[test]
fn fifth_channel_turn_and_seventeenth_owner_turn_are_refused() {
    let capacity = RoutedCapacity::new(RoutedCapacityConfig::default());
    let mut leases = Vec::new();
    for index in 0..4 {
        for _ in 0..4 {
            leases.push(capacity.try_lease(&record(index), "owner").unwrap());
        }
        assert_eq!(
            capacity
                .try_lease(&record(index), "other-owner")
                .unwrap_err(),
            CapacityScope::Channel
        );
    }
    assert_eq!(
        capacity.try_lease(&record(4), "owner").unwrap_err(),
        CapacityScope::Owner
    );
    // Neither failure holds the other scope's permit.
    let other = capacity.try_lease(&record(4), "other-owner").unwrap();
    drop(leases.remove(0));
    let released = capacity.try_lease(&record(0), "owner").unwrap();
    drop((other, released, leases));
    capacity.sweep();
    assert!(capacity.channels.is_empty());
    assert!(capacity.owners.is_empty());
}
#[test]
fn failed_owner_attempts_also_sweep_idle_channel_entries() {
    let capacity = RoutedCapacity::new(RoutedCapacityConfig {
        per_channel: 1,
        per_owner: 1,
    });
    let held = capacity.try_lease(&record(0), "owner").unwrap();
    for index in 1..1024 {
        assert_eq!(
            capacity.try_lease(&record(index), "owner").unwrap_err(),
            CapacityScope::Owner
        );
        assert!(capacity.channels.len() <= 257);
    }
    assert_eq!(capacity.owners.len(), 1);
    drop(held);
    capacity.sweep();
    assert!(capacity.channels.is_empty());
}
#[test]
fn sweep_retains_cloned_semaphores_before_permit_acquisition() {
    let capacity = RoutedCapacity::new(RoutedCapacityConfig::default());
    drop(capacity.try_lease(&record(0), "owner").unwrap());
    let channel = Arc::clone(capacity.channels.get(&record(0)).unwrap().value());
    let owner = Arc::clone(capacity.owners.get("owner").unwrap().value());
    capacity.sweep();
    assert!(Arc::ptr_eq(
        &channel,
        capacity.channels.get(&record(0)).unwrap().value()
    ));
    assert!(Arc::ptr_eq(
        &owner,
        capacity.owners.get("owner").unwrap().value()
    ));
    drop((channel, owner));
    capacity.sweep();
    assert!(capacity.channels.is_empty());
    assert!(capacity.owners.is_empty());
}
#[test]
fn concurrent_acquisition_and_sweeping_never_recreate_live_capacity() {
    use std::sync::atomic::AtomicUsize;
    let capacity = RoutedCapacity::new(RoutedCapacityConfig {
        per_channel: 4,
        per_owner: 4,
    });
    let active = AtomicUsize::new(0);
    let succeeded = AtomicUsize::new(0);
    std::thread::scope(|scope| {
        for _ in 0..8 {
            scope.spawn(|| {
                for _ in 0..2048 {
                    if let Ok(lease) = capacity.try_lease(&record(0), "owner") {
                        assert!(active.fetch_add(1, Ordering::SeqCst) < 4);
                        succeeded.fetch_add(1, Ordering::Relaxed);
                        std::thread::yield_now();
                        active.fetch_sub(1, Ordering::SeqCst);
                        drop(lease);
                    }
                    capacity.sweep();
                }
            });
        }
    });
    assert!(succeeded.load(Ordering::Relaxed) > 0);
    assert_eq!(active.load(Ordering::Relaxed), 0);
    capacity.sweep();
    assert!(capacity.channels.is_empty());
    assert!(capacity.owners.is_empty());
}
#[test]
fn only_complete_leases_increment_the_unlabelled_gauge() {
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    metrics::with_local_recorder(&recorder, || {
        let capacity = RoutedCapacity::new(RoutedCapacityConfig {
            per_channel: 1,
            per_owner: 1,
        });
        let lease = capacity.try_lease(&record(0), "owner").unwrap();
        assert!(capacity.try_lease(&record(0), "other").is_err());
        assert!(capacity.try_lease(&record(1), "owner").is_err());
        assert!(handle
            .render()
            .lines()
            .any(|line| line == "cmaeg_routed_turns_in_flight 1"));
        drop(lease);
        assert!(handle
            .render()
            .lines()
            .any(|line| line == "cmaeg_routed_turns_in_flight 0"));
    });
}
