use super::*;
use crate::allocation_probe::{deallocation::arc_allocation_bytes, AllocationProbe};
use crate::ingress_budget::{ClassBounds, IngressClass, IngressOwnershipPolicy, IngressWorkScope};

fn plan(bytes: usize) -> AllocationPlan {
    AllocationPlan::checked("custody teardown fixture", &[bytes]).unwrap()
}

fn fixture() -> (IngressBudget, IngressOwnershipPolicy) {
    let root = IngressBudget::new(64 * 1024);
    let policy = IngressOwnershipPolicy::new(
        root.clone(),
        ClassBounds::new(root.capacity_bytes(), plan(8192), plan(8192)).unwrap(),
    )
    .unwrap();
    (root, policy)
}

#[test]
fn isolated_shared_metadata_deallocation_contract() {
    const TEST: &str =
        "ingress_budget::ownership::custody_tests::isolated_shared_metadata_deallocation_contract";
    if crate::allocation_probe::deallocation::isolated(TEST) {
        return;
    }
    assert_eq!(
        size_of::<SharedCustody<ReservationBundle>>(),
        size_of::<Arc<ReservationBundle>>()
    );
    let (root, policy) = fixture();
    let bytes = arc_allocation_bytes::<ReservationBundle>();
    let mut probe = AllocationProbe::new();
    let (owner, pause) = probe.pause_deallocation(bytes, || {
        Arc::new(policy.admit(IngressClass::Data, plan(1024)).unwrap())
    });
    assert!(
        !pause.admission_blocked_while_dropping(&policy, IngressClass::Data, move || drop(owner)),
        "the old inside-Arc permit must expose its early release"
    );
    root.assert_idle();
    assert_eq!(probe.live(), 0);

    let (owner, pause) = probe.pause_deallocation(bytes, || {
        policy
            .admit(IngressClass::Data, plan(1024))
            .unwrap()
            .initialize(|| vec![9u8; 256])
    });
    assert!(
        pause.admission_blocked_while_dropping(&policy, IngressClass::Data, move || drop(owner)),
        "shared metadata permits returned before physical deallocation"
    );
    root.assert_idle();
    assert_eq!(probe.live(), 0);

    // All strong owners use the same extraction protocol, including racing drops.
    for _ in 0..32 {
        let (owner, pause) = probe.pause_deallocation(bytes, || {
            policy
                .admit(IngressClass::Data, plan(1024))
                .unwrap()
                .initialize(|| vec![17u8; 256])
        });
        let clone = owner.clone();
        let barrier = Arc::new(std::sync::Barrier::new(2));
        assert!(
            pause.admission_blocked_while_dropping(&policy, IngressClass::Data, move || {
                let other_barrier = barrier.clone();
                let other = std::thread::spawn(move || {
                    other_barrier.wait();
                    drop(clone);
                });
                barrier.wait();
                drop(owner);
                other.join().unwrap();
            })
        );
        root.assert_idle();
        assert_eq!(probe.live(), 0);
    }

    let (scope, pause) = probe.pause_deallocation(bytes, || {
        IngressWorkScope::new(policy.admit(IngressClass::Data, plan(1024)).unwrap())
    });
    scope
        .admit_extra(&policy, IngressClass::Data, plan(2048))
        .unwrap();
    let output = scope.initialize_owned(|| vec![31u8; 512]);
    drop(scope);
    assert!(
        pause.admission_blocked_while_dropping(&policy, IngressClass::Data, move || drop(output))
    );
    root.assert_idle();
    assert_eq!(probe.live(), 0);
}
