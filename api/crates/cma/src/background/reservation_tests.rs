use super::*;

#[test]
fn reserved_recovery_and_shared_work_progress_under_foreground_without_exceeding_two() {
    let shared = BackgroundBudget::new();
    let recovery = shared.reserve(1).unwrap();
    let foreground = shared.foreground();
    let optional = shared.try_enter().unwrap();
    let recovering = recovery.try_enter().unwrap();
    assert!(shared.try_enter().is_none());
    assert!(recovery.try_enter().is_none());
    // Foreground departure cannot introduce a third chain while recovery owns
    // its reserved lane, including while that lane is temporarily idle.
    drop(foreground);
    assert!(shared.try_enter().is_none());
    drop(recovering);
    assert!(shared.try_enter().is_none());
    assert!(recovery.try_enter().is_some());
    drop(optional);
    assert!(shared.try_enter().is_some());
    drop(recovery);
    let first = shared.try_enter().unwrap();
    let second = shared.try_enter().unwrap();
    assert!(shared.try_enter().is_none());
    drop((first, second));
}

#[test]
fn disabled_shared_admission_still_defers_behind_one_foreground_background_chain() {
    let shared = BackgroundBudget::new();
    let foreground = shared.foreground();
    let optional = shared.try_enter().unwrap();
    assert!(shared.try_enter().is_none());
    drop(optional);
    assert!(shared.try_enter().is_some());
    drop(foreground);
    let first = shared.try_enter().unwrap();
    assert!(shared.try_enter().is_some());
    drop(first);
}

#[test]
fn invalid_or_busy_reservations_preserve_capacity_and_clones_retain_ownership() {
    let shared = BackgroundBudget::new();
    for capacity in [0, 2, usize::MAX] {
        assert!(shared.reserve(capacity).is_none());
    }
    let first = shared.try_enter().unwrap();
    let second = shared.try_enter().unwrap();
    assert!(shared.reserve(1).is_none());
    drop((first, second));
    let recovery = shared.reserve(1).unwrap();
    assert!(shared.reserve(1).is_none());
    let retained = recovery.clone();
    drop(recovery);
    let optional = shared.try_enter().unwrap();
    assert!(shared.try_enter().is_none());
    drop(retained);
    assert!(shared.try_enter().is_some());
    drop(optional);
}
