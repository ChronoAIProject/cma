use super::*;
use crate::allocation_probe::AllocationProbe;
use crate::ingress_budget::{ClassBounds, IngressBudget};
use std::sync::Arc;

fn fixture() -> (IngressBudget, IngressOwnershipPolicy) {
    let root = IngressBudget::new(1024 * 1024);
    let floor = AllocationPlan::checked("mutex fixture floor", &[64 * 1024]).unwrap();
    let policy = IngressOwnershipPolicy::new(
        root.clone(),
        ClassBounds::new(root.capacity_bytes(), floor, floor).unwrap(),
    )
    .unwrap();
    // The shared process/class semaphore backing is not part of this mutex.
    drop(policy.admit(IngressClass::Data, floor).unwrap());
    (root, policy)
}

#[test]
fn isolated_mutex_backing_is_admitted_before_first_unlock_and_freed_before_release() {
    const TEST: &str = "hosted::acp::mutex::tests::isolated_mutex_backing_is_admitted_before_first_unlock_and_freed_before_release";
    if std::env::var_os("CMA_BRIDGE_MUTEX_CHILD").is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", TEST, "--nocapture", "--test-threads=1"])
            .env("CMA_BRIDGE_MUTEX_CHILD", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
        eprintln!("{}", String::from_utf8_lossy(&output.stderr));
        return;
    }
    let (root, policy) = fixture();
    let mut probe = AllocationProbe::new();
    let mutex = probe.track(|| BridgeMutex::new(7, Some(&policy)).unwrap());
    let admitted = root.reserved_work_bytes();
    assert!(probe.peak() <= admitted);
    assert!(probe.live() <= admitted);
    let constructed = probe.allocated();
    probe.track(|| *mutex.try_lock().unwrap() = 9);
    assert_eq!(
        probe.allocated(),
        constructed,
        "first caller must not allocate persistent backing"
    );
    assert_eq!(*mutex.try_lock().unwrap(), 9);
    eprintln!(
        "bridge_mutex peak={} retained={} admitted={admitted}",
        probe.peak(),
        probe.live()
    );
    let shared = Arc::new(mutex);
    let last = shared.clone();
    drop(shared);
    assert_eq!(root.reserved_work_bytes(), admitted);
    drop(last);
    assert_eq!(probe.live(), 0);
    root.assert_idle();
    drop(probe);

    let mut probe = AllocationProbe::new();
    {
        let raw = tokio::sync::Mutex::new(0);
        probe.track(|| drop(raw.try_lock().unwrap()));
        #[cfg(target_vendor = "apple")]
        assert_eq!(probe.live(), size_of::<libc::pthread_mutex_t>());
        root.assert_idle();
        eprintln!("raw_bridge_mutex retained={} admitted=0", probe.live());
    }
    assert_eq!(probe.live(), 0);
}

#[test]
fn mutex_refusal_does_not_initialize_backing_and_legacy_locking_is_unchanged() {
    let (root, policy) = fixture();
    let available = policy.budget(IngressClass::Data).available_bytes();
    let pressure = policy
        .admit(
            IngressClass::Data,
            AllocationPlan::checked("mutex pressure", &[available]).unwrap(),
        )
        .unwrap();
    let mut probe = AllocationProbe::new();
    for _ in 0..128 {
        let result = probe.track(|| BridgeMutex::new(0, Some(&policy)));
        #[cfg(target_vendor = "apple")]
        assert!(matches!(result, Err(AdmissionRefusal::AtCapacity { .. })));
        #[cfg(target_os = "linux")]
        assert!(result.is_ok());
    }
    assert_eq!(probe.allocated(), 0);
    drop((probe, pressure));
    let mutex = BridgeMutex::new(3, None).unwrap();
    assert!(mutex._storage.is_none());
    *mutex.try_lock().unwrap() = 5;
    assert_eq!(*mutex.try_lock().unwrap(), 5);
    root.assert_idle();
}
