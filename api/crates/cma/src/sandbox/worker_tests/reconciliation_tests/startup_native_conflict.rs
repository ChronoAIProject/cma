//! Exercise real native adoption/status races without bypassing provider fences.
use super::*;

async fn change_parent(fixture: &NativeStartup, name: &str) -> Sandbox {
    let mut newer = fixture
        .store
        .inner
        .get_sandbox(&fixture.original.id)
        .await
        .unwrap()
        .unwrap();
    let revision = newer.revision;
    newer.revision += 1;
    newer.name = name.into();
    fixture
        .store
        .inner
        .save_sandbox(revision, newer.clone())
        .await
        .unwrap();
    newer
}

fn assert_retained(fixture: &NativeStartup) {
    assert!(!fixture
        .control
        .api
        .lock()
        .unwrap()
        .calls
        .iter()
        .any(|(method, _)| method == "DELETE"));
    assert_eq!(fixture.control.cancelled.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn startup_native_parent_race_retries_read_and_cas_preserving_the_new_parent() {
    for record_boot in [false, true] {
        let fixture = NativeStartup::new(record_boot, false).await;
        let startup = fixture.reconciler.initialize_before_workers(fixture.now);
        tokio::pin!(startup);
        until(&mut startup, fixture.control.status_entered.notified()).await;
        until(&mut startup, fixture.control.siblings_started(7)).await;
        let newer = change_parent(&fixture, "concurrent parent revision").await;
        fixture.control.status_gate.add_permits(1);
        until(&mut startup, fixture.control.status_entered.notified()).await;
        fixture.control.assert_no_cleanup();
        assert_eq!(
            fixture.store.inner.get_sandbox(&newer.id).await.unwrap(),
            Some(newer.clone())
        );
        assert_eq!(fixture.control.other_started.load(Ordering::SeqCst), 7);
        fixture.control.status_gate.add_permits(1);
        until(&mut startup, fixture.control.siblings_started(8)).await;
        fixture.control.assert_no_cleanup();
        fixture.control.other_gate.add_permits(8);
        tokio::time::timeout(Duration::from_secs(10), startup)
            .await
            .unwrap()
            .unwrap();
        let stored = fixture
            .store
            .inner
            .get_sandbox(&newer.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.name, newer.name);
        assert_eq!(stored.revision, newer.revision + u64::from(record_boot));
        assert_eq!(
            stored.runtime.as_ref().unwrap().boot_id.as_deref(),
            Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
        );
        assert_eq!(fixture.control.other_settled.load(Ordering::SeqCst), 8);
        assert!(fixture.control.cleanup.load(Ordering::SeqCst) > 0);
        assert!(!fixture.reconciler.binding_is_held(&newer.id));
        assert_retained(&fixture);
    }
}

#[tokio::test]
async fn startup_native_repeated_parent_conflict_is_bounded_and_protected_until_fresh_retry() {
    for record_boot in [false, true] {
        let fixture = NativeStartup::new(record_boot, false).await;
        let startup = fixture.reconciler.initialize_before_workers(fixture.now);
        tokio::pin!(startup);
        until(&mut startup, fixture.control.status_entered.notified()).await;
        until(&mut startup, fixture.control.siblings_started(7)).await;
        let mut latest = fixture.original.clone();
        for attempt in 1..=3 {
            latest = change_parent(&fixture, &format!("concurrent parent {attempt}")).await;
            fixture.control.status_gate.add_permits(1);
            if attempt < 3 {
                until(&mut startup, fixture.control.status_entered.notified()).await;
            } else {
                until(&mut startup, fixture.control.siblings_started(8)).await;
            }
            fixture.control.assert_no_cleanup();
        }
        fixture.control.other_gate.add_permits(8);
        tokio::time::timeout(Duration::from_secs(10), startup)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            fixture
                .control
                .api
                .lock()
                .unwrap()
                .calls
                .iter()
                .filter(|(kind, _)| kind == "STATUS_ENTER")
                .count(),
            3
        );
        assert_eq!(
            fixture.store.inner.get_sandbox(&latest.id).await.unwrap(),
            Some(latest.clone())
        );
        assert_eq!(fixture.control.other_settled.load(Ordering::SeqCst), 8);
        assert!(fixture.reconciler.binding_is_held(&latest.id));
        fixture.control.assert_no_cleanup();
        assert_retained(&fixture);

        // Advance only the process-local hold backoff; native I/O remains on
        // the real clock and all provider/durable checks run again.
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(31)).await;
        tokio::time::resume();
        fixture.control.other_started.store(0, Ordering::SeqCst);
        fixture.control.other_settled.store(0, Ordering::SeqCst);
        fixture.control.status_gate.add_permits(1);
        fixture.control.other_gate.add_permits(8);
        // Lifecycle setup advances its durable clock ahead of wall time.
        // Preserve that caller floor for this later initialization too.
        let retry_at = SystemTime::now().max(fixture.now);
        tokio::time::timeout(
            Duration::from_secs(10),
            fixture.reconciler.initialize_before_workers(retry_at),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(!fixture.reconciler.binding_is_held(&latest.id));
        assert!(fixture.control.cleanup.load(Ordering::SeqCst) > 0);
        let settled = fixture
            .store
            .inner
            .get_sandbox(&latest.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(settled.name, latest.name);
        assert_eq!(settled.revision, latest.revision + u64::from(record_boot));
        assert_retained(&fixture);
    }
}

#[tokio::test]
async fn startup_native_changed_binding_is_deferred_without_adopting_new_targets() {
    let fixture = NativeStartup::new(false, false).await;
    let startup = fixture.reconciler.initialize_before_workers(fixture.now);
    tokio::pin!(startup);
    until(&mut startup, fixture.control.status_entered.notified()).await;
    until(&mut startup, fixture.control.siblings_started(7)).await;
    let mut newer = fixture.original.clone();
    newer.revision += 1;
    newer
        .runtime
        .as_mut()
        .unwrap()
        .runtime_id
        .push_str("-successor");
    newer.validate().unwrap();
    fixture
        .store
        .inner
        .save_sandbox(fixture.original.revision, newer.clone())
        .await
        .unwrap();
    fixture.control.status_gate.add_permits(1);
    until(&mut startup, fixture.control.siblings_started(8)).await;
    fixture.control.other_gate.add_permits(8);
    tokio::time::timeout(Duration::from_secs(10), startup)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        fixture
            .control
            .api
            .lock()
            .unwrap()
            .calls
            .iter()
            .filter(|(kind, _)| kind == "STATUS_ENTER")
            .count(),
        1
    );
    assert!(fixture.reconciler.binding_is_held(&newer.id));
    assert_eq!(
        fixture.store.inner.get_sandbox(&newer.id).await.unwrap(),
        Some(newer)
    );
    fixture.control.assert_no_cleanup();
    assert_retained(&fixture);
}

#[tokio::test]
async fn missing_native_home_defers_only_a_superseded_binding() {
    assert_missing_home_respects_durable_binding(false, false).await;
}

#[tokio::test]
async fn home_disappearance_during_status_defers_only_a_superseded_binding() {
    assert_missing_home_respects_durable_binding(true, false).await;
}

#[tokio::test]
async fn startup_native_home_refusal_keeps_the_binding_held_when_enabled() {
    for status_in_flight in [false, true] {
        assert_missing_home_respects_durable_binding(status_in_flight, true).await;
    }
}

async fn assert_missing_home_respects_durable_binding(
    status_in_flight: bool,
    native_home_hold_enabled: bool,
) {
    for binding_changed in [false, true] {
        let mut fixture = NativeStartup::new(false, false).await;
        fixture.reconciler = fixture
            .reconciler
            .with_startup_native_home_hold(native_home_hold_enabled);
        fixture
            .control
            .hold_next_home_check
            .store(!status_in_flight, Ordering::SeqCst);
        let startup = fixture.reconciler.initialize_before_workers(fixture.now);
        tokio::pin!(startup);
        if status_in_flight {
            until(&mut startup, fixture.control.status_entered.notified()).await;
        } else {
            until(&mut startup, fixture.control.home_check_entered.notified()).await;
        }
        let mut current = fixture.original.clone();
        current.revision += 1;
        current.name = "updated while checking the home".into();
        if binding_changed {
            current
                .runtime
                .as_mut()
                .unwrap()
                .runtime_id
                .push_str("-successor");
        }
        current.validate().unwrap();
        fixture
            .store
            .inner
            .save_sandbox(fixture.original.revision, current.clone())
            .await
            .unwrap();
        if status_in_flight {
            // Let the provider finish validating the status exchange before
            // removing the PVC at the follow-up hosted home verification.
            fixture
                .control
                .hold_next_home_check
                .store(true, Ordering::SeqCst);
            fixture.control.status_gate.add_permits(1);
            until(&mut startup, fixture.control.home_check_entered.notified()).await;
        }
        fixture.control.api.lock().unwrap().rows.remove(api::VOLUME);
        fixture.control.other_gate.add_permits(8);
        fixture.control.home_check_gate.add_permits(1);
        let result = tokio::time::timeout(Duration::from_secs(10), startup)
            .await
            .unwrap();
        if binding_changed || native_home_hold_enabled {
            result.unwrap();
            assert!(fixture.reconciler.binding_is_held(&current.id));
            assert_eq!(fixture.control.other_settled.load(Ordering::SeqCst), 8);
        } else {
            let error = result.unwrap_err();
            assert!(
                matches!(&error, SandboxStoreError::Unavailable(message)
                if message.contains("native home identity changed or disappeared")),
                "{error}"
            );
        }
        assert_eq!(
            fixture.store.inner.get_sandbox(&current.id).await.unwrap(),
            Some(current)
        );
        fixture.control.assert_no_cleanup();
        assert!(!fixture
            .control
            .api
            .lock()
            .unwrap()
            .calls
            .iter()
            .any(|(kind, _)| kind == "DURABLE_CAS"));
        assert_retained(&fixture);
    }
}

#[tokio::test]
async fn startup_native_refreshes_first_attempt_after_authority_advanced_past_startup_time() {
    let fixture = NativeStartup::new(false, false).await;
    let durable_at = stamp_time(&fixture.original.updated_at);
    let startup_floor = durable_at - Duration::from_secs(1);
    // Lifecycle fixture timestamps can be ahead of the real clock. Let wall time
    // catch up so this exercises stale startup time, not a future caller floor.
    after_durable_boundary(&fixture.original).await;
    assert!(SystemTime::now() >= durable_at);

    let startup = fixture.reconciler.initialize_before_workers(startup_floor);
    tokio::pin!(startup);
    until(&mut startup, fixture.control.status_entered.notified()).await;
    until(&mut startup, fixture.control.siblings_started(7)).await;
    fixture.control.assert_no_cleanup();
    fixture.control.status_gate.add_permits(1);
    until(&mut startup, fixture.control.siblings_started(8)).await;
    fixture.control.other_gate.add_permits(8);
    tokio::time::timeout(Duration::from_secs(10), startup)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(
        fixture
            .store
            .inner
            .get_sandbox(&fixture.original.id)
            .await
            .unwrap(),
        Some(fixture.original.clone())
    );
    assert!(!fixture.reconciler.binding_is_held(&fixture.original.id));
    assert_eq!(fixture.control.other_settled.load(Ordering::SeqCst), 8);
    assert!(fixture.control.cleanup.load(Ordering::SeqCst) > 0);
    assert_retained(&fixture);
}
