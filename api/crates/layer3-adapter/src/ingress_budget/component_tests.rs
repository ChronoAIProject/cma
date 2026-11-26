use super::*;
use crate::allocation_probe::AllocationProbe;
use crate::ingress_budget::{
    sizing, AllocationPlan, ClassBounds, IngressBudget, IngressClass, IngressOwnershipPolicy,
};
use tokio::sync::Semaphore;

pub(super) fn plan(bytes: usize) -> AllocationPlan {
    AllocationPlan::checked("component owner fixture", &[bytes]).unwrap()
}

pub(super) fn policy() -> (IngressBudget, IngressOwnershipPolicy) {
    let root = IngressBudget::new(64 * 1024);
    let bounds = ClassBounds::new(root.capacity_bytes(), plan(8192), plan(8192))
        .unwrap()
        .with_attachment_limit(1)
        .unwrap();
    let policy = IngressOwnershipPolicy::new(root.clone(), bounds).unwrap();
    drop(policy.admit(IngressClass::Data, plan(1)).unwrap());
    (root, policy)
}

pub(super) fn component(bytes: usize) -> Arc<Semaphore> {
    let component = Arc::new(Semaphore::new(bytes));
    // Isolate the separately resident component mutex backing on Apple.
    drop(IngressReservation::try_acquire(&component, None, 1).unwrap());
    component
}

#[repr(align(64))]
struct Aligned([u8; 64]);
#[repr(align(128))]
struct AlignedEmpty;

fn check_layout<T>(value: T) {
    let bytes = sizing::component_shared_owner_bytes::<T>().unwrap();
    let component = component(bytes);
    let reservation = IngressReservation::try_acquire(&component, None, bytes).unwrap();
    let mut probe = AllocationProbe::new();
    let owner = probe.track(|| reservation.initialize_shared(|| value));
    assert_eq!(probe.allocated(), bytes);
    assert_eq!(probe.live(), bytes);
    assert_eq!(component.available_permits(), 0);
    let cloned = probe.track(|| owner.clone());
    assert_eq!(probe.allocated(), bytes, "cloning must allocate nothing");
    assert!(std::ptr::eq(&*owner, &*cloned));
    drop(owner);
    assert_eq!(component.available_permits(), 0);
    drop(cloned);
    assert_eq!(component.available_permits(), bytes);
    assert_eq!(probe.live(), 0);
}

#[test]
fn fixed_layout_matches_actual_allocations_and_nonclone_payload_clones() {
    check_layout(());
    check_layout(1u8);
    check_layout(3u128);
    let aligned = Aligned([7; 64]);
    assert_eq!(aligned.0[63], 7);
    check_layout(aligned);
    check_layout(AlignedEmpty);
}

#[test]
fn component_only_owner_can_cross_threads_without_cloning_the_payload() {
    struct Value(u32);
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<SharedComponentOwned<Value>>();
    let bytes = sizing::component_shared_owner_bytes::<Value>().unwrap();
    let component = component(bytes);
    let owner = IngressReservation::try_acquire(&component, None, bytes)
        .unwrap()
        .initialize_shared(|| Value(42));
    let cloned = owner.clone();
    drop(owner);
    std::thread::spawn(move || assert_eq!(cloned.0, 42))
        .join()
        .unwrap();
    assert_eq!(component.available_permits(), bytes);
}

#[test]
fn aggregate_transfer_preserves_original_debits_and_attachment_through_last_clone() {
    let (root, policy) = policy();
    let idle = root.available_bytes();
    let data = policy.budget(IngressClass::Data);
    let control = policy.budget(IngressClass::Control).available_bytes();
    let transformation = policy
        .budget(IngressClass::Transformation)
        .available_bytes();
    let attachment = policy.admit_attachment().unwrap();
    let attached = root.available_bytes();
    let bytes = sizing::component_shared_owner_bytes::<Vec<u8>>().unwrap() + 257;
    let component = component(bytes);
    let storage = IngressReservation::try_acquire(
        &component,
        Some(&attachment.policy().budget(IngressClass::Data)),
        bytes,
    )
    .unwrap();
    let debits = (
        root.available_bytes(),
        data.available_bytes(),
        component.available_permits(),
    );
    let owner = storage.initialize_shared(|| {
        assert_eq!(root.available_bytes(), attached - bytes);
        assert_eq!(component.available_permits(), 0);
        vec![11u8; 257]
    });
    assert_eq!(
        (
            root.available_bytes(),
            data.available_bytes(),
            component.available_permits()
        ),
        debits
    );
    let clone = owner.clone();
    drop((attachment, owner));
    assert!(policy.admit_attachment().is_err());
    assert_eq!(clone[256], 11);
    assert_eq!(
        policy.budget(IngressClass::Control).available_bytes(),
        control
    );
    assert_eq!(
        policy
            .budget(IngressClass::Transformation)
            .available_bytes(),
        transformation
    );
    assert_eq!(root.available_bytes(), attached - bytes);
    drop(clone);
    assert_eq!(root.available_bytes(), idle);
    assert_eq!(component.available_permits(), bytes);
    drop(policy.admit_attachment().unwrap());
    root.assert_idle();
}

#[test]
fn failed_acquisition_never_initializes_or_keeps_partial_permits() {
    let (root, policy) = policy();
    let data = policy.budget(IngressClass::Data);
    let bytes = sizing::component_shared_owner_bytes::<()>().unwrap();
    let component = component(bytes);
    let held = IngressReservation::try_acquire(&component, Some(&data), bytes).unwrap();
    let before = root.available_bytes();
    let mut probe = AllocationProbe::new();
    let refusal = probe.track(|| {
        IngressReservation::try_acquire(&component, Some(&data), 1)
            .map(|permit| permit.initialize_shared(|| panic!("refused component initialized")))
    });
    assert!(refusal.is_none());
    assert_eq!(probe.allocated(), 0);
    assert_eq!(root.available_bytes(), before);
    drop(held);
    let occupied = policy
        .admit(IngressClass::Data, plan(data.available_bytes()))
        .unwrap();
    let before = root.available_bytes();
    let refusal = probe.track(|| {
        IngressReservation::try_acquire(&component, Some(&data), bytes)
            .map(|permit| permit.initialize_shared(|| panic!("refused aggregate initialized")))
    });
    assert!(refusal.is_none());
    assert_eq!(probe.allocated(), 0);
    assert_eq!(component.available_permits(), bytes);
    assert_eq!(root.available_bytes(), before);
    drop(occupied);
    root.assert_idle();
}

#[test]
fn allocating_initializer_unwinds_partial_work_before_returning_reservation() {
    struct Partial<'a> {
        bytes: Vec<u8>,
        component: &'a Semaphore,
        root: &'a IngressBudget,
        held: usize,
        dropped: &'a std::sync::atomic::AtomicBool,
    }
    impl Drop for Partial<'_> {
        fn drop(&mut self) {
            assert_eq!(self.component.available_permits(), 0);
            assert_eq!(self.root.available_bytes(), self.held);
            // The partial allocation is released while the incoming permit is held.
            drop(std::mem::take(&mut self.bytes));
            assert_eq!(self.component.available_permits(), 0);
            assert_eq!(self.root.available_bytes(), self.held);
            self.dropped
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }
    let (root, policy) = policy();
    let bytes = sizing::component_shared_owner_bytes::<Vec<u8>>().unwrap() + 257;
    let component = component(bytes);
    let reservation = IngressReservation::try_acquire(
        &component,
        Some(&policy.budget(IngressClass::Data)),
        bytes,
    )
    .unwrap();
    let held = root.available_bytes();
    let dropped = std::sync::atomic::AtomicBool::new(false);
    let unwind = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        reservation.initialize_shared(|| {
            let _partial = Partial {
                bytes: vec![9u8; 257],
                component: &component,
                root: &root,
                held,
                dropped: &dropped,
            };
            panic!("synthetic allocating initializer");
        })
    }));
    assert!(unwind.is_err());
    assert!(dropped.load(std::sync::atomic::Ordering::SeqCst));
    assert_eq!(component.available_permits(), bytes);
    root.assert_idle();
}
