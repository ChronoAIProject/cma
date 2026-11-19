use super::*;
use crate::allocation_probe::{deallocation, AllocationProbe};
use crate::ingress_budget::{sizing, ClassBounds, IngressClass, IngressOwnershipPolicy};

#[test]
fn isolated_heap_initialization_error_keeps_custody() {
    if deallocation::isolated(
        "ingress_budget::ownership::initialization_tests::isolated_heap_initialization_error_keeps_custody",
    ) {
        return;
    }
    let root = IngressBudget::new(64 * 1024);
    let plan = |bytes| AllocationPlan::checked("initializer error fixture", &[bytes]).unwrap();
    let policy = IngressOwnershipPolicy::new(
        root.clone(),
        ClassBounds::new(root.capacity_bytes(), plan(8192), plan(8192)).unwrap(),
    )
    .unwrap();
    let storage = policy
        .admit(
            IngressClass::Data,
            AllocationPlan::checked(
                "partial initialization and heap error",
                &[
                    512,
                    4096,
                    sizing::fallible_shared_owner_bytes::<(), Vec<u8>>(),
                ],
            )
            .unwrap(),
        )
        .unwrap();
    let mut probe = AllocationProbe::new();
    let error = probe
        .track(|| {
            storage.try_initialize(|| {
                let partial = vec![17u8; 512];
                let error = vec![31u8; 4096];
                assert_eq!(partial[511], 17);
                drop(partial);
                Err::<(), Vec<u8>>(error)
            })
        })
        .unwrap_err();
    drop(policy);
    assert_eq!(error.len(), 4096);
    let charged = root.capacity_bytes() - root.available_bytes();
    assert!(probe.live() >= 4096);
    assert!(
        probe.live() <= charged,
        "heap error remains allocated after initialization: {} live > {charged} charged",
        probe.live()
    );
    assert!(probe.peak() <= charged);
    let allocated = probe.allocated();
    let clone = probe.track(|| error.clone());
    assert!(clone.ptr_eq(&error));
    assert_eq!(probe.allocated(), allocated);
    drop(error);
    assert_eq!(root.capacity_bytes() - root.available_bytes(), charged);
    assert!(probe.live() >= 4096);
    drop(clone);
    assert_eq!(probe.live(), 0);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}

#[test]
fn static_failure_releases_partial_storage_without_allocating_an_error_owner() {
    let root = IngressBudget::new(8192);
    let plan = |n| AllocationPlan::checked("static initialization fixture", &[n]).unwrap();
    let policy = IngressOwnershipPolicy::new(
        root.clone(),
        ClassBounds::new(8192, plan(1024), plan(2048)).unwrap(),
    )
    .unwrap();
    // Shared semaphore backing belongs to the policy and precedes this probe.
    drop(policy.admit(IngressClass::Data, plan(1)).unwrap());
    let mut probe = AllocationProbe::new();
    let refusal = AdmissionRefusal::InvalidPlan {
        phase: "synthetic decode",
    };
    let error = probe
        .track(|| {
            policy
                .admit(IngressClass::Data, plan(1024))
                .unwrap()
                .try_initialize_static(|| {
                    let partial = vec![19u8; 512];
                    assert_eq!(partial[511], 19);
                    drop(partial);
                    Err::<Vec<u8>, _>(refusal)
                })
        })
        .unwrap_err();
    assert_eq!(error, refusal);
    assert_eq!(probe.allocated(), 512);
    assert_eq!(probe.live(), 0);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
    let success = probe
        .track(|| {
            policy
                .admit(IngressClass::Data, plan(1024))
                .unwrap()
                .try_initialize_static(|| Ok(vec![23u8; 512]))
        })
        .unwrap();
    assert_eq!(success[511], 23);
    assert!(probe.live() <= root.capacity_bytes() - root.available_bytes());
    drop(success);
    assert_eq!(probe.live(), 0);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}

#[test]
fn fallible_owner_layout_covers_either_returned_branch() {
    #[derive(Debug)]
    #[repr(align(64))]
    struct AlignedError([u8; 64]);
    let root = IngressBudget::new(8192);
    let plan = |n| AllocationPlan::checked("fallible layout fixture", &[n]).unwrap();
    let policy = IngressOwnershipPolicy::new(
        root.clone(),
        ClassBounds::new(8192, plan(1024), plan(2048)).unwrap(),
    )
    .unwrap();
    let bytes = sizing::fallible_shared_owner_bytes::<u8, AlignedError>();
    assert!(bytes >= sizing::shared_owner_bytes::<u8>());
    assert!(bytes >= sizing::shared_owner_bytes::<AlignedError>());
    let mut probe = AllocationProbe::new();
    let error = probe
        .track(|| {
            policy
                .admit(IngressClass::Data, plan(bytes))
                .unwrap()
                .try_initialize(|| Err::<u8, _>(AlignedError([29; 64])))
        })
        .unwrap_err();
    assert_eq!(error.0[63], 29);
    assert!(probe.live() <= bytes);
    drop(error);
    assert_eq!(probe.live(), 0);
    let success = probe
        .track(|| {
            policy
                .admit(IngressClass::Data, plan(bytes))
                .unwrap()
                .try_initialize(|| Ok::<_, AlignedError>(31u8))
        })
        .unwrap();
    assert_eq!(*success, 31);
    assert!(probe.live() <= bytes);
    drop(success);
    assert_eq!(probe.live(), 0);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}
