use super::*;

#[test]
fn desktop_epochs_cover_open_completion_drop_and_exclusive_does_not_change_history() {
    let id = SandboxId::generate();
    let baseline = runtime_desktop_activity_snapshot(&id).unwrap();
    assert_eq!(baseline.opening_or_open, 0);
    let desktop = try_acquire_desktop_runtime_guard(&id).unwrap();
    let opening = runtime_desktop_activity_snapshot(&id).unwrap();
    assert_eq!(opening.activity_epoch, baseline.activity_epoch + 1);
    assert_eq!(opening.opening_or_open, 1);
    assert!(try_acquire_runtime_migration_guard(&id).is_err());
    desktop.opened().unwrap();
    assert_eq!(
        runtime_desktop_activity_snapshot(&id)
            .unwrap()
            .activity_epoch,
        baseline.activity_epoch + 2
    );
    drop(desktop);
    let closed = runtime_desktop_activity_snapshot(&id).unwrap();
    assert_eq!(closed.activity_epoch, baseline.activity_epoch + 3);
    assert_eq!(closed.opening_or_open, 0);
    let migration = try_acquire_runtime_migration_guard(&id).unwrap();
    assert_eq!(runtime_desktop_activity_snapshot(&id).unwrap(), closed);
    assert!(try_acquire_desktop_runtime_guard(&id).is_err());
    drop(migration);
    assert_eq!(runtime_desktop_activity_snapshot(&id).unwrap(), closed);
}

#[test]
fn bounded_tracker_eviction_resets_continuity_and_cannot_evict_referenced_entries() {
    let mut registry = Trackers::default();
    let id = SandboxId::generate();
    let first = registry.get(&id, 1).unwrap();
    let before = first.activity.lock().unwrap().clone().unwrap();
    let other = SandboxId::generate();
    assert!(registry.get(&other, 1).is_err());
    drop(first);
    drop(registry.get(&other, 1).unwrap());
    let after = registry
        .get(&id, 1)
        .unwrap()
        .activity
        .lock()
        .unwrap()
        .clone()
        .unwrap();
    assert_eq!(before.process_incarnation, after.process_incarnation);
    assert_ne!(before.tracking_incarnation, after.tracking_incarnation);
    assert_eq!(after.activity_epoch, 0);
}

#[test]
fn desktop_epoch_overflow_invalidates_tracking_and_migration_instead_of_wrapping() {
    let id = SandboxId::generate();
    let tracker = tracker(&id).unwrap();
    tracker
        .activity
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .activity_epoch = u64::MAX;
    assert!(try_acquire_desktop_runtime_guard(&id).is_err());
    assert!(runtime_desktop_activity_snapshot(&id).is_err());
    assert!(try_acquire_runtime_migration_guard(&id).is_err());
}

#[test]
fn desktop_drop_overflow_is_unknown_and_never_a_quiet_snapshot() {
    let id = SandboxId::generate();
    let desktop = try_acquire_desktop_runtime_guard(&id).unwrap();
    desktop
        .tracker
        .activity
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .activity_epoch = u64::MAX;
    drop(desktop);
    assert!(runtime_desktop_activity_snapshot(&id).is_err());
    assert!(try_acquire_runtime_migration_guard(&id).is_err());
}
