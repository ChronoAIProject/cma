use super::*;
#[path = "startup_fixture.rs"]
mod fixture;
use fixture::fixture;

#[tokio::test(start_paused = true)]
async fn startup_thirty_four_bindings_finish_before_orphan_and_home_cleanup() {
    for (exclusive, peak, seconds) in [(false, 1, 102), (true, 8, 15)] {
        let fixture = fixture(34, Some(34), None, Duration::from_secs(3)).await;
        let started = tokio::time::Instant::now();
        if exclusive {
            fixture
                .reconciler
                .initialize_before_workers(fixture.now)
                .await
                .unwrap();
        } else {
            fixture.reconciler.initialize(fixture.now).await.unwrap();
        }
        assert_eq!(started.elapsed(), Duration::from_secs(seconds));
        assert_eq!(fixture.control.started.load(Ordering::SeqCst), 34);
        assert_eq!(fixture.control.settled.load(Ordering::SeqCst), 34);
        assert_eq!(fixture.control.peak.load(Ordering::SeqCst), peak);
        assert_eq!(fixture.control.cleanup.load(Ordering::SeqCst), 1);
        assert_eq!(fixture.control.homes.load(Ordering::SeqCst), 1);
        assert_eq!(fixture.control.cancelled.load(Ordering::SeqCst), 0);
    }
}

/// A non-transient runtime refusal still stops startup; see `transient` for the
/// unreachable-runtime case that defers one binding instead.
#[tokio::test]
async fn startup_error_waits_for_started_adoptions_and_never_enters_cleanup() {
    let fixture = fixture(12, Some(12), Some(2), Duration::ZERO).await;
    let startup = fixture.reconciler.initialize_before_workers(fixture.now);
    tokio::pin!(startup);
    assert!(futures_util::poll!(&mut startup).is_pending());
    assert_eq!(fixture.control.started.load(Ordering::SeqCst), 8);
    fixture.control.gates[2].add_permits(1);
    assert!(futures_util::poll!(&mut startup).is_pending());
    assert_eq!(fixture.control.settled.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.control.started.load(Ordering::SeqCst), 8);
    assert_eq!(fixture.control.cleanup.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.control.homes.load(Ordering::SeqCst), 0);
    for (index, gate) in fixture.control.gates.iter().take(8).enumerate() {
        if index != 2 {
            gate.add_permits(1);
        }
    }
    assert!(startup.await.is_err());
    assert_eq!(fixture.control.started.load(Ordering::SeqCst), 8);
    assert_eq!(fixture.control.settled.load(Ordering::SeqCst), 8);
    assert_eq!(fixture.control.cancelled.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.control.cleanup.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.control.homes.load(Ordering::SeqCst), 0);
}

#[tokio::test(start_paused = true)]
async fn startup_full_batch_capacity_and_exclusive_context_select_the_real_path() {
    for (headroom, exclusive, expected_peak, reason) in [
        (Some(9), true, 8, "none"),
        (Some(8), true, 1, "capacity_limited"),
        (None, true, 1, "capacity_unknown"),
        (Some(100), false, 1, "nonexclusive_context"),
    ] {
        let fixture = fixture(9, headroom, None, Duration::from_millis(1)).await;
        assert_eq!(
            fixture.reconciler.startup_binding_schedule(
                &fixture.authority,
                &fixture.inventory,
                exclusive
            ),
            (expected_peak, reason)
        );
        if exclusive {
            fixture
                .reconciler
                .initialize_before_workers(fixture.now)
                .await
                .unwrap();
        } else {
            fixture.reconciler.initialize(fixture.now).await.unwrap();
        }
        assert_eq!(fixture.control.peak.load(Ordering::SeqCst), expected_peak);
        assert_eq!(fixture.control.settled.load(Ordering::SeqCst), 9);
        assert_eq!(fixture.control.cleanup.load(Ordering::SeqCst), 1);
        assert_eq!(fixture.control.homes.load(Ordering::SeqCst), 1);
    }
}

#[path = "startup_targets.rs"]
mod targets;
#[path = "startup_transient.rs"]
mod transient;
