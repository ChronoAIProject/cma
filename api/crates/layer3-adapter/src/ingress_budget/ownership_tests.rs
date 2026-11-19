use super::*;
use crate::ingress_budget::{
    ClassBounds, IngressClass, IngressOwnershipPolicy, IngressReservation,
};
use tokio::sync::Semaphore;

fn plan(bytes: usize) -> AllocationPlan {
    AllocationPlan::new("test", &[bytes]).unwrap()
}

fn policy() -> (IngressBudget, IngressOwnershipPolicy) {
    let root = IngressBudget::new(8192);
    let policy = IngressOwnershipPolicy::new(
        root.clone(),
        ClassBounds::new(8192, plan(1024), plan(2048)).unwrap(),
    )
    .unwrap();
    (root, policy)
}

#[test]
fn data_saturation_preserves_real_control_and_transformation_allocations() {
    let (root, policy) = policy();
    let data = policy
        .reserve(IngressClass::Data, plan(5120))
        .unwrap()
        .initialize(|| vec![7u8; 4000]);
    assert!(policy.reserve(IngressClass::Data, plan(1)).is_err());
    let control = policy
        .reserve(IngressClass::Control, plan(1024))
        .unwrap()
        .initialize(|| "refused".repeat(32));
    let work = policy
        .reserve(IngressClass::Transformation, plan(2048))
        .unwrap()
        .initialize(|| data.iter().map(|n| n + 1).take(1024).collect::<Vec<_>>());
    assert_eq!(control.len(), 224);
    assert_eq!(work[0], 8);
    assert_eq!(root.available_bytes(), 0);
    drop((control, work, data));
    assert_eq!(root.available_bytes(), 8192);
}

#[test]
fn split_and_shared_clone_hold_payload_until_last_owner() {
    let (root, policy) = policy();
    let mut bundle = policy.reserve(IngressClass::Data, plan(4096)).unwrap();
    assert!(bundle.split(plan(4097)).is_err());
    assert_eq!(bundle.requested_bytes(), 4096);
    let child = bundle.split(plan(2048)).unwrap();
    let owner = child.initialize(|| vec![42u8; 1024]);
    let clone = owner.clone();
    assert!(clone.ptr_eq(&owner));
    drop((owner, bundle));
    assert_eq!(root.available_bytes(), 6144);
    assert_eq!(clone[1023], 42);
    drop(clone);
    assert_eq!(root.available_bytes(), 8192);
}

#[test]
fn repeated_policy_and_existing_component_share_the_same_class_ceiling() {
    let (root, policy) = policy();
    let duplicate = IngressOwnershipPolicy::new(root.clone(), policy.bounds()).unwrap();
    let component = Arc::new(Semaphore::new(8192));
    let held =
        IngressReservation::try_acquire(&component, Some(&policy.budget(IngressClass::Data)), 5120)
            .unwrap();
    assert!(duplicate.reserve(IngressClass::Data, plan(1)).is_err());
    assert!(IngressReservation::try_acquire(
        &component,
        Some(&duplicate.budget(IngressClass::Data)),
        1,
    )
    .is_none());
    assert_eq!(component.available_permits(), 3072);
    assert!(IngressOwnershipPolicy::new(
        root.clone(),
        ClassBounds::new(8192, plan(512), plan(2048)).unwrap(),
    )
    .is_err());
    drop(held);
    assert_eq!(root.available_bytes(), 8192);
}

#[test]
fn overflow_refusal_and_initialization_failure_release_every_allowance() {
    assert!(AllocationPlan::new("overflow", &[usize::MAX, 1]).is_err());
    assert!(crate::ingress_budget::sizing::array_bytes::<u64>(usize::MAX).is_err());
    assert!(ClassBounds::new(100, plan(50), plan(50)).is_err());
    let (root, policy) = policy();
    let error = policy
        .reserve(IngressClass::Data, plan(1024))
        .unwrap()
        .try_initialize_static(|| {
            Err::<Vec<u8>, _>(AdmissionRefusal::InvalidPlan { phase: "malformed" })
        });
    assert_eq!(
        error.unwrap_err(),
        AdmissionRefusal::InvalidPlan { phase: "malformed" }
    );
    assert_eq!(root.available_bytes(), 8192);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        policy
            .reserve(IngressClass::Data, plan(1024))
            .unwrap()
            .initialize(|| panic!("decode unwound"));
    }));
    assert!(result.is_err());
    assert_eq!(root.available_bytes(), 8192);
}

#[test]
fn payload_destructor_runs_before_reservation_release() {
    struct Observe(IngressBudget);
    impl Drop for Observe {
        fn drop(&mut self) {
            assert_eq!(self.0.available_bytes(), 3072);
        }
    }
    let (root, policy) = policy();
    let owner = policy
        .reserve(IngressClass::Data, plan(5120))
        .unwrap()
        .initialize(|| Observe(root.clone()));
    drop(owner);
    assert_eq!(root.available_bytes(), 8192);
}

#[test]
fn independent_growth_reserves_old_plus_new_until_old_capacity_is_freed() {
    let (root, policy) = policy();
    let old = policy
        .reserve(IngressClass::Data, plan(4096))
        .unwrap()
        .initialize(|| vec![1u8; 2048]);
    assert!(policy.reserve(IngressClass::Data, plan(4096)).is_err());
    assert_eq!(old.len(), 2048);
    let working = policy
        .reserve(IngressClass::Transformation, plan(2048))
        .unwrap();
    let new = working.initialize(|| old.iter().take(1024).copied().collect::<Vec<_>>());
    assert_eq!(root.available_bytes(), 2048);
    drop(old);
    assert_eq!(root.available_bytes(), 6144);
    assert_eq!(new.len(), 1024);
    drop(new);
    assert_eq!(root.available_bytes(), 8192);
}
