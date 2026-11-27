use super::tests::{component, plan, policy};
use super::*;
use crate::allocation_probe::{deallocation, AllocationProbe};
use crate::ingress_budget::{sizing, IngressBudget, IngressClass, IngressOwnershipPolicy};
use std::sync::Barrier;
use tokio::sync::Semaphore;

const PAYLOAD: usize = 257;

struct Fixture {
    root: IngressBudget,
    policy: IngressOwnershipPolicy,
    component: Arc<Semaphore>,
    bytes: usize,
    idle: usize,
    held_root: usize,
    held_data: usize,
}

impl Fixture {
    fn prepare<T>() -> (Self, IngressReservation) {
        let (root, policy) = policy();
        let idle = root.available_bytes();
        let bytes = sizing::component_shared_owner_bytes::<T>().unwrap() + PAYLOAD;
        let component = component(bytes);
        let attachment = policy.admit_attachment().unwrap();
        let storage = IngressReservation::try_acquire(
            &component,
            Some(&attachment.policy().budget(IngressClass::Data)),
            bytes,
        )
        .unwrap();
        drop(attachment);
        let held_root = root.available_bytes();
        let held_data = policy.budget(IngressClass::Data).available_bytes();
        (
            Self {
                root,
                policy,
                component,
                bytes,
                idle,
                held_root,
                held_data,
            },
            storage,
        )
    }

    fn assert_held(&self) {
        assert_eq!(self.root.available_bytes(), self.held_root);
        assert_eq!(
            self.policy.budget(IngressClass::Data).available_bytes(),
            self.held_data
        );
        assert_eq!(self.component.available_permits(), 0);
        assert!(self.root.try_reserve(self.held_root + 1).is_none());
        assert!(self
            .policy
            .admit(IngressClass::Data, plan(self.held_data + 1))
            .is_err());
        assert!(IngressReservation::try_acquire(&self.component, None, 1).is_none());
        assert!(self.policy.admit_attachment().is_err());
    }

    fn assert_released(&self) {
        assert_eq!(self.root.available_bytes(), self.idle);
        assert_eq!(self.component.available_permits(), self.bytes);
        drop(self.root.try_reserve(self.held_root + 1).unwrap());
        drop(
            self.policy
                .admit(IngressClass::Data, plan(self.held_data + 1))
                .unwrap(),
        );
        drop(IngressReservation::try_acquire(&self.component, None, 1).unwrap());
        drop(self.policy.admit_attachment().unwrap());
        self.root.assert_idle();
    }
}

#[derive(Clone, Copy)]
enum Boundary {
    Dynamic,
    Payload,
    Custody,
}

impl Boundary {
    fn bytes(self) -> usize {
        match self {
            Self::Dynamic => PAYLOAD,
            Self::Payload => deallocation::arc_allocation_bytes::<Vec<u8>>(),
            Self::Custody => deallocation::arc_allocation_bytes::<IngressReservation>(),
        }
    }

    fn address(self, owner: &SharedComponentOwned<Vec<u8>>) -> usize {
        match self {
            Self::Dynamic => owner.as_ptr() as usize,
            Self::Payload => std::ptr::from_ref(&**owner) as usize,
            Self::Custody => std::ptr::from_ref(&*owner.storage) as usize,
        }
    }
}

fn physical_free_contract(racing: bool) {
    for boundary in [Boundary::Dynamic, Boundary::Payload, Boundary::Custody] {
        for _ in 0..if racing { 16 } else { 1 } {
            let (fixture, storage) = Fixture::prepare::<Vec<u8>>();
            let mut probe = AllocationProbe::new();
            let (owner, pause) = probe.pause_deallocation_containing(
                boundary.bytes(),
                || storage.initialize_shared(|| vec![7u8; PAYLOAD]),
                |owner| boundary.address(owner),
            );
            let cloned = owner.clone();
            let barrier = Arc::new(Barrier::new(2));
            pause.inspect_while_dropping(
                move || {
                    if racing {
                        let other_barrier = barrier.clone();
                        let other = std::thread::spawn(move || {
                            other_barrier.wait();
                            drop(cloned);
                        });
                        barrier.wait();
                        drop(owner);
                        other.join().unwrap();
                    } else {
                        drop(owner);
                        drop(cloned);
                    }
                },
                || fixture.assert_held(),
            );
            assert_eq!(
                probe.allocated(),
                fixture.bytes,
                "exact one-owner allowance"
            );
            assert_eq!(probe.live(), 0);
            fixture.assert_released();
        }
    }
}

#[test]
fn isolated_component_owner_physical_free_retains_all_permits() {
    const TEST: &str = "ingress_budget::component::deallocation_tests::isolated_component_owner_physical_free_retains_all_permits";
    if deallocation::isolated(TEST) {
        return;
    }
    physical_free_contract(false);
}

#[test]
fn isolated_local_component_owner_retains_permits_through_physical_free() {
    const TEST: &str = "ingress_budget::component::deallocation_tests::isolated_local_component_owner_retains_permits_through_physical_free";
    if deallocation::isolated(TEST) {
        return;
    }
    for boundary in [Boundary::Dynamic, Boundary::Payload, Boundary::Custody] {
        let bytes = sizing::component_shared_owner_bytes::<Vec<u8>>().unwrap() + PAYLOAD;
        let component = component(bytes);
        let storage = IngressReservation::try_acquire(&component, None, bytes).unwrap();
        let mut probe = AllocationProbe::new();
        let (owner, pause) = probe.pause_deallocation_containing(
            boundary.bytes(),
            || storage.initialize_shared(|| vec![7u8; PAYLOAD]),
            |owner| boundary.address(owner),
        );
        let last = owner.clone();
        drop(owner);
        pause.inspect_while_dropping(
            move || drop(last),
            || {
                assert_eq!(component.available_permits(), 0);
                assert!(IngressReservation::try_acquire(&component, None, 1).is_none());
            },
        );
        assert_eq!(probe.allocated(), bytes);
        assert_eq!(probe.live(), 0);
        assert_eq!(component.available_permits(), bytes);
    }
}

#[test]
fn isolated_component_owner_racing_final_clones_retain_all_permits() {
    const TEST: &str = "ingress_budget::component::deallocation_tests::isolated_component_owner_racing_final_clones_retain_all_permits";
    if deallocation::isolated(TEST) {
        return;
    }
    physical_free_contract(true);
}

#[test]
fn isolated_plain_component_arc_exposes_early_permit_release() {
    const TEST: &str = "ingress_budget::component::deallocation_tests::isolated_plain_component_arc_exposes_early_permit_release";
    if deallocation::isolated(TEST) {
        return;
    }
    struct Legacy {
        _value: Arc<Vec<u8>>,
        storage: Arc<IngressReservation>,
    }
    let (fixture, storage) = Fixture::prepare::<Vec<u8>>();
    let bytes = deallocation::arc_allocation_bytes::<IngressReservation>();
    let mut probe = AllocationProbe::new();
    let (owner, pause) = probe.pause_deallocation_containing(
        bytes,
        || Legacy {
            _value: Arc::new(vec![7u8; PAYLOAD]),
            storage: Arc::new(storage),
        },
        |owner| std::ptr::from_ref(&*owner.storage) as usize,
    );
    pause.inspect_while_dropping(
        move || drop(owner),
        || {
            // This must expose the old defect with exactly the corrected allowance.
            fixture.assert_released();
        },
    );
    assert_eq!(probe.allocated(), fixture.bytes);
    assert_eq!(probe.live(), 0);
}

#[test]
fn isolated_initialization_unwind_frees_partial_payload_before_permits() {
    const TEST: &str = "ingress_budget::component::deallocation_tests::isolated_initialization_unwind_frees_partial_payload_before_permits";
    if deallocation::isolated(TEST) {
        return;
    }
    let (fixture, storage) = Fixture::prepare::<Vec<u8>>();
    let mut probe = AllocationProbe::new();
    let (partial, pause) = probe.pause_deallocation_containing(
        PAYLOAD,
        || vec![3u8; PAYLOAD],
        |partial| partial.as_ptr() as usize,
    );
    pause.inspect_while_dropping(
        move || {
            let unwind = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                storage.initialize_shared(|| {
                    let _partial = partial;
                    panic!("synthetic partial initialization");
                })
            }));
            assert!(unwind.is_err());
        },
        || fixture.assert_held(),
    );
    assert_eq!(probe.live(), 0);
    fixture.assert_released();
}

#[test]
fn isolated_payload_unwind_frees_both_backings_before_permits() {
    const TEST: &str = "ingress_budget::component::deallocation_tests::isolated_payload_unwind_frees_both_backings_before_permits";
    if deallocation::isolated(TEST) {
        return;
    }
    struct Panicking {
        _bytes: Vec<u8>,
    }
    impl Drop for Panicking {
        fn drop(&mut self) {
            panic!("synthetic payload destructor");
        }
    }
    for custody in [false, true] {
        let (fixture, storage) = Fixture::prepare::<Panicking>();
        let bytes = if custody {
            deallocation::arc_allocation_bytes::<IngressReservation>()
        } else {
            deallocation::arc_allocation_bytes::<Panicking>()
        };
        let mut probe = AllocationProbe::new();
        let (owner, pause) = probe.pause_deallocation_containing(
            bytes,
            || {
                storage.initialize_shared(|| Panicking {
                    _bytes: vec![5u8; PAYLOAD],
                })
            },
            |owner| {
                if custody {
                    std::ptr::from_ref(&*owner.storage) as usize
                } else {
                    std::ptr::from_ref(&**owner) as usize
                }
            },
        );
        pause.inspect_while_dropping(
            move || {
                assert!(
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(owner))).is_err()
                );
            },
            || fixture.assert_held(),
        );
        assert_eq!(probe.allocated(), fixture.bytes);
        assert_eq!(probe.live(), 0);
        fixture.assert_released();
    }
}
