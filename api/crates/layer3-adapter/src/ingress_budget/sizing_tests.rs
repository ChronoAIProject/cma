use super::*;
use crate::allocation_probe::AllocationProbe;
use crate::ingress_budget::{IngressBudget, IngressOwnershipPolicy};

#[repr(align(64))]
struct Aligned {
    _bytes: [u8; 64],
}

#[repr(align(128))]
struct AlignedEmpty;

#[test]
fn checked_arc_layout_preserves_existing_header_and_padding() {
    #[repr(C, align(2))]
    struct Previous<T> {
        _strong: std::sync::atomic::AtomicUsize,
        _weak: std::sync::atomic::AtomicUsize,
        _value: T,
    }
    fn compare<T>() {
        let checked = shared_allocation_layout(Layout::new::<T>()).unwrap();
        assert_eq!(checked, Layout::new::<Previous<T>>());
        assert_eq!(shared_allocation_bytes::<T>(), checked.size());
        assert_eq!(
            checked.size(),
            crate::allocation_probe::deallocation::arc_allocation_bytes::<T>()
        );
    }
    compare::<()>();
    compare::<u8>();
    compare::<u128>();
    compare::<[u8; 127]>();
    compare::<Aligned>();
    compare::<AlignedEmpty>();
    compare::<super::super::IngressReservation>();
    compare::<super::super::ReservationBundle>();
}

#[test]
fn checked_component_layout_rejects_unsupported_sizes_without_allocating() {
    let mut probe = AllocationProbe::new();
    probe.track(|| {
        assert!(
            shared_allocation_layout(Layout::from_size_align(isize::MAX as usize, 1).unwrap())
                .is_err()
        );
        #[cfg(target_pointer_width = "64")]
        {
            assert!(matches!(
                component_shared_owner_bytes::<[u8; u32::MAX as usize]>(),
                Err(super::super::AdmissionRefusal::InvalidPlan { .. })
            ));
        }
        #[cfg(all(
            target_pointer_width = "64",
            any(target_os = "linux", target_vendor = "apple")
        ))]
        {
            // T is compiler-valid; an Arc-layout<T> wrapper type would exceed
            // the compiler's object-size limit. Never instantiate that wrapper.
            type Huge = [u8; (1usize << 61) - 8];
            assert!(shared_allocation_layout(Layout::new::<Huge>()).is_ok());
            assert!(matches!(
                component_shared_owner_bytes::<Huge>(),
                Err(super::super::AdmissionRefusal::InvalidPlan { .. })
            ));
        }
    });
    assert_eq!(probe.allocated(), 0);
}

#[test]
fn shared_owner_manifest_matches_pinned_arc_layout_for_empty_and_aligned_values() {
    check(());
    check(7u8);
    check(11u128);
    check(Aligned { _bytes: [0; 64] });
    check(AlignedEmpty);
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
