use super::fixture::{fixture_with, Fixture};
use super::*;

fn held(fixture: &Fixture) -> usize {
    fixture
        .authority
        .keys()
        .filter(|id| fixture.reconciler.binding_held(id.as_str()))
        .count()
}

#[tokio::test(start_paused = true)]
async fn startup_defers_one_unreachable_runtime_and_the_sweep_recovers_it() {
    for detail in [
        "Kubernetes runtime stream opening timed out",
        "runtime assignment timed out",
    ] {
        let fixture =
            fixture_with(12, Some(12), Some(2), Some(detail), Duration::from_secs(1)).await;
        fixture
            .reconciler
            .initialize_before_workers(fixture.now)
            .await
            .unwrap();
        // Admission stayed open: every other binding was still reconciled.
        assert_eq!(fixture.control.started.load(Ordering::SeqCst), 12);
        assert_eq!(fixture.control.settled.load(Ordering::SeqCst), 12);
        assert_eq!(fixture.control.cancelled.load(Ordering::SeqCst), 0);
        // One deferred binding leaves every startup cleanup to the sweep.
        assert_eq!(fixture.control.cleanup.load(Ordering::SeqCst), 0);
        assert_eq!(fixture.control.homes.load(Ordering::SeqCst), 0);
        assert_eq!(held(&fixture), 1, "{detail}");

        // Before the hold's backoff, the sweep still cleans up what is not held.
        fixture.control.sweeping.store(true, Ordering::SeqCst);
        let mut finished = false;
        for _ in 0..30 {
            if fixture.reconciler.run_due(SystemTime::now()).await.unwrap() {
                finished = true;
                break;
            }
        }
        assert!(finished);
        assert_eq!(held(&fixture), 1);
        assert_eq!(fixture.control.cleanup.load(Ordering::SeqCst), 1);
        assert_eq!(fixture.control.homes.load(Ordering::SeqCst), 1);

        // After the backoff, the sweep's fresh attempt adopts it and drops the hold.
        tokio::time::advance(Duration::from_secs(31)).await;
        let mut finished = false;
        for _ in 0..30 {
            if fixture.reconciler.run_due(SystemTime::now()).await.unwrap() {
                finished = true;
                break;
            }
        }
        assert!(finished);
        assert_eq!(held(&fixture), 0, "{detail}");
    }
}

#[tokio::test(start_paused = true)]
async fn startup_still_stops_on_a_non_transient_runtime_refusal() {
    let fixture = fixture_with(12, Some(12), Some(2), None, Duration::from_secs(1)).await;
    let error = fixture
        .reconciler
        .initialize_before_workers(fixture.now)
        .await
        .unwrap_err();
    assert!(matches!(error, SandboxStoreError::Unavailable(ref message)
        if message.contains("runtime rejected request")));
    assert_eq!(fixture.control.cleanup.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.control.homes.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn startup_native_home_hold_does_not_cover_the_binding_inventory_read() {
    let fixture = fixture_with(1, Some(1), None, None, Duration::ZERO).await;
    fixture
        .control
        .refuse_binding_inventory
        .store(true, Ordering::SeqCst);
    let error = fixture
        .reconciler
        .with_startup_native_home_hold(true)
        .initialize_before_workers(fixture.now)
        .await
        .unwrap_err();
    assert!(matches!(error, SandboxStoreError::Unavailable(ref message)
        if message.ends_with("native home identity changed or disappeared")));
    assert_eq!(fixture.control.inventory_calls.load(Ordering::SeqCst), 2);
    assert_eq!(fixture.control.started.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.control.cleanup.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.control.homes.load(Ordering::SeqCst), 0);
}
