use super::*;
use crate::allocation_probe::{deallocation, AllocationProbe};

#[test]
fn root_bootstrap_remains_charged_after_class_metadata_is_freed() {
    let mut probe = AllocationProbe::new();
    let root = probe.track(|| IngressBudget::new(64 * 1024));
    let bootstrap = root.bootstrap_bytes();
    let observer = root.0.clone();
    let floor = AllocationPlan::checked("root test floor", &[8192]).unwrap();
    let bounds = ClassBounds::new(root.capacity_bytes(), floor, floor).unwrap();
    let policy = probe.track(|| IngressOwnershipPolicy::new(root.clone(), bounds).unwrap());
    assert!(probe.live() > 0);
    drop((root, policy));
    assert!(probe.live() > 0, "the root semaphore still exists");
    assert_eq!(
        observer.available_permits(),
        bounds.total_bytes() - bootstrap,
        "the root cannot return its own allocation's allowance"
    );
    drop(observer);
    assert_eq!(probe.live(), 0);
}

#[test]
fn cold_root_and_lazy_backing_fit_the_fixed_bootstrap_without_changing_legacy_capacity() {
    let mut probe = AllocationProbe::new();
    let root = probe.track(|| IngressBudget::new(64 * 1024));
    assert_eq!(root.available_bytes(), root.capacity_bytes());
    assert_eq!(root.metadata_bytes(), 0);
    assert!(probe.live() > 0);
    probe.track(|| drop(root.0.clone().try_acquire_owned().unwrap()));
    assert!(probe.peak() <= root.bootstrap_bytes());
    eprintln!(
        "root bootstrap={} cold peak={}",
        root.bootstrap_bytes(),
        probe.peak()
    );
    let clone = root.clone();
    drop(root);
    assert!(probe.live() > 0);
    assert_eq!(clone.available_bytes(), clone.capacity_bytes());
    drop(clone);
    assert_eq!(probe.live(), 0);
}

#[test]
fn cold_policy_construction_and_all_lazy_locks_fit_one_resident_manifest() {
    let mut probe = AllocationProbe::new();
    let (root, policy) = probe.track(|| {
        let root = IngressBudget::new(64 * 1024);
        let floor = AllocationPlan::checked("cold root test floor", &[8192]).unwrap();
        let bounds = ClassBounds::new(root.capacity_bytes(), floor, floor).unwrap();
        let policy = IngressOwnershipPolicy::new_admitted(root.clone(), bounds).unwrap();
        for class in [
            IngressClass::Data,
            IngressClass::Transformation,
            IngressClass::Control,
        ] {
            drop(
                policy
                    .admit(
                        class,
                        AllocationPlan::checked("first lock use", &[1]).unwrap(),
                    )
                    .unwrap(),
            );
        }
        (root, policy)
    });
    let metadata = root.bootstrap_bytes() + classes::storage_bytes().unwrap();
    assert_eq!(root.metadata_bytes(), metadata);
    assert_eq!(policy.bounds().metadata_bytes(), metadata);
    assert!(probe.peak() <= metadata);
    assert_eq!(root.capacity_bytes() - root.available_bytes(), metadata);
    eprintln!("resident metadata={metadata} cold peak={}", probe.peak());
    let before = probe.allocated();
    probe.track(|| {
        for _ in 0..32 {
            drop(IngressOwnershipPolicy::new_admitted(root.clone(), policy.bounds()).unwrap());
        }
    });
    assert_eq!(probe.allocated(), before);
    root.assert_idle();
    drop((root, policy));
    assert_eq!(probe.live(), 0);
}

#[test]
fn isolated_process_bootstrap_has_one_root_and_one_resident_charge() {
    const TEST: &str = "ingress_budget::root_storage_tests::isolated_process_bootstrap_has_one_root_and_one_resident_charge";
    if deallocation::isolated(TEST) {
        return;
    }
    let mut probe = AllocationProbe::new();
    let root = probe.track(IngressBudget::process);
    assert!(probe.peak() > 0, "observe the cold process constructor");
    assert!(probe.peak() <= root.bootstrap_bytes());
    assert_eq!(root.available_bytes(), root.capacity_bytes());
    let floor = AllocationPlan::checked("process floor", &[8192]).unwrap();
    let bounds = ClassBounds::new(root.capacity_bytes(), floor, floor).unwrap();
    let policy =
        probe.track(|| IngressOwnershipPolicy::new_admitted(root.clone(), bounds).unwrap());
    let allocated = probe.allocated();
    probe.track(|| {
        for _ in 0..32 {
            let again = IngressBudget::process();
            assert!(Arc::ptr_eq(&root.0, &again.0));
            assert!(Arc::ptr_eq(&root.1, &again.1));
            drop(IngressOwnershipPolicy::new_admitted(again, bounds).unwrap());
        }
    });
    assert_eq!(probe.allocated(), allocated);
    assert_eq!(
        root.capacity_bytes() - root.available_bytes(),
        bounds.metadata_bytes()
    );
    assert!(probe.peak() <= bounds.metadata_bytes());
    drop((root, policy));
    assert!(
        probe.live() > 0,
        "the process singleton retains its provision"
    );
}

#[test]
fn isolated_policy_deallocation_keeps_its_own_root_debit() {
    const TEST: &str =
        "ingress_budget::root_storage_tests::isolated_policy_deallocation_keeps_its_own_root_debit";
    if deallocation::isolated(TEST) {
        return;
    }
    for concurrent in [false, true] {
        policy_deallocation(concurrent);
    }
}

fn policy_deallocation(concurrent: bool) {
    let mut probe = AllocationProbe::new();
    let ((root, policy), pause) = probe.pause_deallocation_containing(
        sizing::shared_allocation_bytes::<BudgetPolicy>(),
        || {
            let root = IngressBudget::new(64 * 1024);
            let floor = AllocationPlan::checked("deallocation floor", &[8192]).unwrap();
            let bounds = ClassBounds::new(root.capacity_bytes(), floor, floor).unwrap();
            let policy = IngressOwnershipPolicy::new_admitted(root.clone(), bounds).unwrap();
            (root, policy)
        },
        |(root, _)| Arc::as_ptr(&root.1) as usize,
    );
    let observer = root.0.clone();
    let total = root.capacity_bytes();
    let bootstrap = root.bootstrap_bytes();
    let peer = concurrent.then(|| policy.clone());
    pause.inspect_while_dropping(
        move || {
            if let Some(peer) = peer {
                let barrier = Arc::new(std::sync::Barrier::new(2));
                let other_barrier = barrier.clone();
                let other = std::thread::spawn(move || {
                    other_barrier.wait();
                    drop(peer);
                });
                barrier.wait();
                drop((root, policy));
                other.join().unwrap();
            } else {
                drop((root, policy));
            }
        },
        || {
            assert_eq!(observer.available_permits(), total - bootstrap);
            assert!(observer
                .clone()
                .try_acquire_many_owned((total - bootstrap + 1) as u32)
                .is_err());
        },
    );
    assert_eq!(observer.available_permits(), total - bootstrap);
    drop(observer);
    assert_eq!(probe.live(), 0);
}
