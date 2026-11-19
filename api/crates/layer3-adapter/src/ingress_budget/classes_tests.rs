use super::*;
use crate::allocation_probe::{deallocation, AllocationProbe};
use crate::ingress_budget::{IngressClass, IngressOwnershipPolicy, IngressReservation};
use std::sync::Barrier;

fn bounds(root: &IngressBudget) -> ClassBounds {
    let floor = AllocationPlan::checked("class test floor", &[8192]).unwrap();
    ClassBounds::new(root.capacity_bytes(), floor, floor).unwrap()
}

fn root() -> IngressBudget {
    let root = IngressBudget::new(64 * 1024);
    // The existing root's lazy backing is a separate process-metadata boundary.
    drop(root.0.clone().try_acquire_owned().unwrap());
    root
}

#[test]
fn class_allocations_and_lazy_locks_fit_the_pre_admitted_resident_allowance() {
    let root = root();
    let bounds = bounds(&root);
    assert_eq!(root.metadata_bytes(), 0);
    let mut probe = AllocationProbe::new();
    let policy = probe.track(|| {
        let policy = IngressOwnershipPolicy::new(root.clone(), bounds).unwrap();
        for class in [
            IngressClass::Data,
            IngressClass::Control,
            IngressClass::Transformation,
        ] {
            drop(
                policy
                    .admit(
                        class,
                        AllocationPlan::checked("initialize class lock", &[1]).unwrap(),
                    )
                    .unwrap(),
            );
        }
        policy
    });
    assert!(probe.allocated() > 0);
    assert!(probe.peak() <= bounds.metadata_bytes());
    assert_eq!(
        root.capacity_bytes() - root.available_bytes(),
        bounds.metadata_bytes()
    );
    assert_eq!(
        bounds.data_bytes()
            + bounds.control_bytes()
            + bounds.transformation_bytes()
            + bounds.metadata_bytes(),
        root.capacity_bytes()
    );
    drop(policy);
    root.assert_idle();
    assert!(probe.live() > 0, "root retains its installed class pools");
    drop(root);
    assert_eq!(probe.live(), 0);
}

#[test]
fn class_capacity_refusal_allocates_nothing_and_can_retry() {
    let root = root();
    let bounds = bounds(&root);
    let pressure = root
        .0
        .clone()
        .try_acquire_many_owned(root.capacity_bytes() as u32)
        .unwrap();
    let mut probe = AllocationProbe::new();
    assert!(matches!(
        probe.track(|| IngressOwnershipPolicy::new_admitted(root.clone(), bounds)),
        Err(AdmissionRefusal::AtCapacity {
            phase: "class pools"
        })
    ));
    assert_eq!(probe.allocated(), 0);
    assert!(root.1.classes.get().is_none());
    assert_eq!(root.metadata_bytes(), 0);
    drop(pressure);
    install(&root, bounds).unwrap();
    root.assert_idle();
    assert_eq!(root.metadata_bytes(), bounds.metadata_bytes());
}

#[test]
fn repeated_and_racing_initializers_leave_one_resident_pool_set() {
    let root = root();
    let bounds = bounds(&root);
    let barrier = Arc::new(Barrier::new(17));
    let mut workers = Vec::new();
    for _ in 0..16 {
        let root = root.clone();
        let barrier = barrier.clone();
        workers.push(std::thread::spawn(move || {
            barrier.wait();
            IngressOwnershipPolicy::new_admitted(root, bounds)
        }));
    }
    barrier.wait();
    let policies: Vec<_> = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect();
    assert!(policies.iter().any(Result::is_ok));
    for error in policies.iter().filter_map(|result| result.as_ref().err()) {
        assert!(matches!(
            error,
            AdmissionRefusal::AtCapacity {
                phase: "class pool initialization"
            }
        ));
    }
    root.assert_idle();
    assert_eq!(root.metadata_bytes(), bounds.metadata_bytes());
    let mut probe = AllocationProbe::new();
    drop(probe.track(|| IngressOwnershipPolicy::new(root.clone(), bounds).unwrap()));
    assert_eq!(probe.allocated(), 0);
    drop(policies);
    root.assert_idle();
}

#[test]
fn concurrent_initialization_refuses_before_allocating_or_consuming_progress_floors() {
    let root = root();
    let bounds = bounds(&root);
    let initializing = Initialization::claim(&root.1.initializing_classes).unwrap();
    let mut probe = AllocationProbe::new();
    assert!(matches!(
        probe.track(|| IngressOwnershipPolicy::new_admitted(root.clone(), bounds)),
        Err(AdmissionRefusal::AtCapacity {
            phase: "class pool initialization"
        })
    ));
    assert_eq!(root.available_bytes(), root.capacity_bytes());
    assert_eq!(probe.allocated(), 0);
    drop(initializing);
    install(&root, bounds).unwrap();
    root.assert_idle();
}

#[test]
fn isolated_class_storage_survives_split_component_and_final_semaphore_deallocation() {
    const TEST: &str = "ingress_budget::classes::tests::isolated_class_storage_survives_split_component_and_final_semaphore_deallocation";
    if deallocation::isolated(TEST) {
        return;
    }
    let root = root();
    let bounds = bounds(&root);
    let observer = root.0.clone();
    let component = Arc::new(Semaphore::new(256));
    drop(component.clone().try_acquire_owned().unwrap());
    let mut probe = AllocationProbe::new();
    let ((policy, mut storage, component_storage), pause) = probe.pause_deallocation_containing(
        deallocation::arc_allocation_bytes::<Semaphore>(),
        || {
            let policy = IngressOwnershipPolicy::new(root.clone(), bounds).unwrap();
            let storage = policy
                .admit(
                    IngressClass::Data,
                    AllocationPlan::checked("retained class owner", &[128]).unwrap(),
                )
                .unwrap();
            let component_storage = IngressReservation::try_acquire(
                &component,
                Some(&policy.budget(IngressClass::Data)),
                128,
            )
            .unwrap();
            (policy, storage, component_storage)
        },
        |(policy, _, _)| Arc::as_ptr(&policy.root.1.classes.get().unwrap().data) as usize,
    );
    let child = storage
        .split_admitted(AllocationPlan::checked("split class owner", &[64]).unwrap())
        .unwrap();
    drop((root, policy, storage));
    assert_eq!(
        observer.available_permits(),
        bounds.total_bytes() - bounds.metadata_bytes() - 192
    );
    drop(component_storage);
    assert_eq!(
        observer.available_permits(),
        bounds.total_bytes() - bounds.metadata_bytes() - 64
    );
    pause.inspect_while_dropping(
        move || drop(child),
        || {
            assert_eq!(
                observer.available_permits(),
                bounds.total_bytes() - bounds.metadata_bytes()
            );
            let reuse = observer.clone().try_acquire_many_owned(
                (bounds.total_bytes() - bounds.metadata_bytes() + 1) as u32,
            );
            assert!(
                reuse.is_err(),
                "class metadata cannot be reused before its semaphore is freed"
            );
        },
    );
    assert_eq!(observer.available_permits(), bounds.total_bytes());
    assert_eq!(probe.live(), 0);
}

#[test]
fn isolated_last_class_handle_keeps_metadata_through_semaphore_deallocation() {
    const TEST: &str = "ingress_budget::classes::tests::isolated_last_class_handle_keeps_metadata_through_semaphore_deallocation";
    if deallocation::isolated(TEST) {
        return;
    }
    for class in [
        IngressClass::Data,
        IngressClass::Control,
        IngressClass::Transformation,
    ] {
        let root = root();
        let bounds = bounds(&root);
        let observer = root.0.clone();
        let mut probe = AllocationProbe::new();
        let (budget, pause) = probe.pause_deallocation_containing(
            deallocation::arc_allocation_bytes::<Semaphore>(),
            || {
                IngressOwnershipPolicy::new(root.clone(), bounds)
                    .unwrap()
                    .budget(class)
            },
            |budget| Arc::as_ptr(budget.2.as_ref().unwrap()) as usize,
        );
        drop(root);
        pause.inspect_while_dropping(
            move || drop(budget),
            || {
                assert_eq!(
                    observer.available_permits(),
                    bounds.total_bytes() - bounds.metadata_bytes()
                );
            },
        );
        assert_eq!(observer.available_permits(), bounds.total_bytes());
        assert_eq!(probe.live(), 0);
    }
}

#[test]
fn uncharged_class_pool_control_leaves_live_storage_with_full_capacity_available() {
    let root = root();
    let mut probe = AllocationProbe::new();
    let pools = probe.track(|| {
        [
            Arc::new(Semaphore::new(1024)),
            Arc::new(Semaphore::new(1024)),
            Arc::new(Semaphore::new(1024)),
        ]
    });
    assert!(probe.live() > 0);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
    let reused = root
        .0
        .clone()
        .try_acquire_many_owned(root.capacity_bytes() as u32)
        .unwrap();
    assert!(probe.live() > 0);
    drop(pools);
    assert_eq!(probe.live(), 0);
    drop(reused);
}
