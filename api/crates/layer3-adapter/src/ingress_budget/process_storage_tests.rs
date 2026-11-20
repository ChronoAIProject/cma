use super::*;
use crate::allocation_probe::AllocationProbe;
use crate::ingress_budget::{AdmissionRefusal, IngressBudget, IngressOwnershipPolicy};
use std::sync::Arc;
use tokio::sync::Semaphore;

#[test]
fn fixed_process_storage_rolls_back_refusal_and_keeps_one_lifetime_charge() {
    let root = IngressBudget::new(64 * 1024);
    let floor = AllocationPlan::checked("process storage floor", &[8192]).unwrap();
    let original = ClassBounds::new(root.capacity_bytes(), floor, floor).unwrap();
    let process = semaphore_storage_bytes().unwrap();
    let bounds = original
        .with_process_storage(AllocationPlan::checked("fixed fixture pool", &[process]).unwrap())
        .unwrap();
    assert_eq!(bounds.control_bytes(), original.control_bytes());
    assert_eq!(
        bounds.transformation_bytes(),
        original.transformation_bytes()
    );
    assert_eq!(bounds.data_bytes() + process, original.data_bytes());

    let occupied = root.capacity_bytes() - bounds.metadata_bytes() + 1;
    let storage = root.try_reserve(occupied).unwrap();
    let payload = vec![0u8; occupied];
    let mut probe = AllocationProbe::new();
    assert!(matches!(
        probe.track(|| IngressOwnershipPolicy::new_admitted(root.clone(), bounds)),
        Err(AdmissionRefusal::AtCapacity {
            phase: "class pools"
        })
    ));
    assert_eq!(probe.allocated(), 0);
    assert_eq!(root.metadata_bytes(), 0);
    drop((payload, storage));
    assert_eq!(root.available_bytes(), root.capacity_bytes());

    let policy = IngressOwnershipPolicy::new_admitted(root.clone(), bounds).unwrap();
    let component = probe.track(|| {
        let component = Arc::new(Semaphore::new(1024));
        drop(component.clone().try_acquire_owned().unwrap());
        component
    });
    assert!(probe.live() > 0);
    assert!(probe.peak() <= process);
    let allocated = probe.allocated();
    probe.track(|| {
        for _ in 0..16 {
            drop(IngressOwnershipPolicy::new_admitted(root.clone(), bounds).unwrap());
        }
        assert!(matches!(
            IngressOwnershipPolicy::new_admitted(root.clone(), original),
            Err(AdmissionRefusal::InvalidPlan {
                phase: "conflicting class bounds"
            })
        ));
    });
    assert_eq!(probe.allocated(), allocated);
    root.assert_idle();
    let observer = root.0.clone();
    let bootstrap = root.bootstrap_bytes();
    drop((policy, root));
    assert_eq!(
        observer.available_permits(),
        bounds.total_bytes() - bootstrap - process
    );
    assert!(probe.live() > 0, "fixed process storage outlives work");
    drop(component);
    assert_eq!(probe.live(), 0);
    drop(observer);
}

#[test]
fn fixed_process_storage_refuses_floor_exhaustion_and_arithmetic_limits() {
    let floor = AllocationPlan::checked("checked process floor", &[8192]).unwrap();
    let original = ClassBounds::new(64 * 1024, floor, floor).unwrap();
    for bytes in [original.data_bytes(), u32::MAX as usize] {
        let plan = AllocationPlan::checked("oversized process provision", &[bytes]).unwrap();
        assert!(original.with_process_storage(plan).is_err());
    }
    let empty = AllocationPlan::checked("no process provision", &[]).unwrap();
    assert_eq!(original.with_process_storage(empty).unwrap(), original);
    assert_eq!(original.process_storage_bytes(), 0);
}
