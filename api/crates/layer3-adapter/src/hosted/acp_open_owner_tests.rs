use super::*;
use crate::allocation_probe::{deallocation, AllocationProbe};
use crate::ingress_budget::{AllocationPlan, IngressBudget};

fn fixture() -> (Bridge, IngressBudget, IngressOwnershipPolicy) {
    let fixture = super::super::ingress::fixture::fixture();
    drop((fixture.acknowledgements, fixture.writes));
    (fixture.bridge, fixture.root, fixture.policy)
}

#[test]
fn outer_bridge_refuses_without_payload_allocation_after_budget_initialization() {
    for invalid in 0..3 {
        let (bridge, root, policy) = fixture();
        let (foreign_bridge, foreign_root, foreign) = fixture();
        drop(foreign_bridge);
        // Root/class semaphore backing is separate process metadata. Initialize
        // it before measuring this outer-payload admission and teardown.
        for class in [
            IngressClass::Data,
            IngressClass::Control,
            IngressClass::Transformation,
        ] {
            drop(
                policy
                    .admit(
                        class,
                        AllocationPlan::checked("budget initialization", &[1]).unwrap(),
                    )
                    .unwrap(),
            );
        }
        let pressure = (invalid == 0).then(|| {
            policy
                .admit(
                    IngressClass::Data,
                    AllocationPlan::checked(
                        "outer bridge pressure",
                        &[policy.budget(IngressClass::Data).available_bytes()],
                    )
                    .unwrap(),
                )
                .unwrap()
        });
        let selected = match invalid {
            0 => Some(&policy),
            1 => None,
            _ => Some(&foreign),
        };
        let mut probe = AllocationProbe::new();
        let error = probe.track(|| Admitted::own(bridge, selected).err().unwrap());
        if invalid == 0 {
            assert!(matches!(error, AdmissionRefusal::AtCapacity { .. }));
        } else {
            assert!(matches!(error, AdmissionRefusal::InvalidPlan { .. }));
        }
        assert_eq!(probe.allocated(), 0);
        drop(pressure);
        assert_eq!(root.available_bytes(), root.capacity_bytes());
        assert_eq!(
            foreign_root.available_bytes(),
            foreign_root.capacity_bytes()
        );
    }
}

#[test]
fn isolated_legacy_outer_bridge_control_returns_capacity_before_freeing_its_arc() {
    const TEST: &str = "hosted::acp::channel::owner_tests::isolated_legacy_outer_bridge_control_returns_capacity_before_freeing_its_arc";
    if deallocation::isolated(TEST) {
        return;
    }
    let (bridge, root, policy) = fixture();
    let mut probe = AllocationProbe::new();
    let (bridge, pause) = probe.pause_deallocation_containing(
        deallocation::arc_allocation_bytes::<Bridge>(),
        || Legacy::own(bridge, None).unwrap(),
        |bridge| (&**bridge as *const Bridge) as usize,
    );
    pause.inspect_while_dropping(
        move || drop(bridge),
        || {
            assert_eq!(root.available_bytes(), root.capacity_bytes());
            let all_data = policy.admit(
                IngressClass::Data,
                AllocationPlan::checked(
                    "legacy unowned backing control",
                    &[policy.bounds().data_bytes()],
                )
                .unwrap(),
            );
            assert!(all_data.is_ok(), "the old outer Arc has no reservation");
        },
    );
    assert_eq!(probe.live(), 0);
}

#[test]
fn isolated_outer_bridge_stays_charged_after_children_drop_until_its_deallocation() {
    const TEST: &str = "hosted::acp::channel::owner_tests::isolated_outer_bridge_stays_charged_after_children_drop_until_its_deallocation";
    if deallocation::isolated(TEST) {
        return;
    }
    for concurrent in [false, true] {
        let (bridge, root, policy) = fixture();
        let mut probe = AllocationProbe::new();
        let (owner, pause) = probe.pause_deallocation_containing(
            deallocation::arc_allocation_bytes::<Bridge>(),
            || Admitted::own(bridge, Some(&policy)).unwrap(),
            |owner| (&**owner as *const Bridge) as usize,
        );
        let bytes = owner.requested_bytes();
        assert!(bytes >= deallocation::arc_allocation_bytes::<Bridge>());
        let last = owner.clone();
        pause.inspect_while_dropping(
            move || {
                if concurrent {
                    let barrier = Arc::new(std::sync::Barrier::new(2));
                    let other = barrier.clone();
                    let child = std::thread::spawn(move || {
                        other.wait();
                        drop(owner);
                    });
                    barrier.wait();
                    drop(last);
                    child.join().unwrap();
                } else {
                    drop(owner);
                    drop(last);
                }
            },
            || {
                assert_eq!(root.capacity_bytes() - root.available_bytes(), bytes);
                let _pressure = policy
                    .admit(
                        IngressClass::Data,
                        AllocationPlan::checked(
                            "outer bridge remaining capacity",
                            &[policy.bounds().data_bytes() - bytes],
                        )
                        .unwrap(),
                    )
                    .unwrap();
                assert!(policy
                    .admit(
                        IngressClass::Data,
                        AllocationPlan::checked("outer bridge deallocation sentinel", &[1])
                            .unwrap(),
                    )
                    .is_err());
            },
        );
        assert_eq!(probe.live(), 0);
        assert_eq!(root.available_bytes(), root.capacity_bytes());
    }
}

#[test]
fn outer_bridge_custody_survives_clone_and_releases_on_unwind() {
    let (bridge, root, policy) = fixture();
    let mut probe = AllocationProbe::new();
    let owner = probe.track(|| Admitted::own(bridge, Some(&policy)).unwrap());
    let bytes = owner.requested_bytes();
    assert!(probe.live() >= size_of::<Bridge>() && probe.live() <= bytes);
    let last = owner.clone();
    drop(owner);
    assert!(root.capacity_bytes() - root.available_bytes() >= bytes);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        let _owner = last;
        panic!("synthetic bridge owner unwind");
    }));
    assert!(result.is_err());
    assert_eq!(probe.live(), 0);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}
