use super::*;
use crate::allocation_probe::AllocationProbe;
use crate::ingress_budget::{IngressBudget, IngressOwnershipPolicy};

#[repr(align(64))]
struct Aligned {
    _bytes: [u8; 64],
}

#[test]
fn shared_owner_manifest_matches_pinned_arc_layout_for_empty_and_aligned_values() {
    check(());
    check(7u8);
    check(11u128);
    check(Aligned { _bytes: [0; 64] });
}

fn check<T>(value: T) {
    let root = IngressBudget::new(64 * 1024);
    let floor = AllocationPlan::checked("layout test floor", &[8192]).unwrap();
    let policy = IngressOwnershipPolicy::new_admitted(
        root.clone(),
        ClassBounds::new(root.capacity_bytes(), floor, floor).unwrap(),
    )
    .unwrap();
    drop(
        policy
            .admit(
                IngressClass::Data,
                AllocationPlan::checked("resident pool lock", &[1]).unwrap(),
            )
            .unwrap(),
    );
    let bytes = shared_owner_bytes::<T>();
    let storage = policy
        .admit(
            IngressClass::Data,
            AllocationPlan::checked("shared owner layout", &[bytes]).unwrap(),
        )
        .unwrap();
    let mut probe = AllocationProbe::new();
    let owner = probe.track(|| storage.initialize(|| value));
    assert_eq!(probe.allocated(), bytes, "count actual Arc padding once");
    assert_eq!(probe.live(), bytes);
    assert_eq!(owner.requested_bytes(), bytes);
    drop(owner);
    assert_eq!(probe.live(), 0);
    root.assert_idle();
}
