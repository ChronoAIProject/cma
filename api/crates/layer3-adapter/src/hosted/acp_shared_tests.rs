use super::*;
use crate::allocation_probe::{deallocation, AllocationProbe};
use crate::ingress_budget::{ClassBounds, IngressBudget};
use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};

fn policy() -> (IngressBudget, IngressOwnershipPolicy) {
    let root = IngressBudget::new(64 * 1024);
    let floor = AllocationPlan::checked("shared fixture floor", &[4096]).unwrap();
    let policy = IngressOwnershipPolicy::new(
        root.clone(),
        ClassBounds::new(root.capacity_bytes(), floor, floor).unwrap(),
    )
    .unwrap();
    drop(policy.admit(IngressClass::Data, floor).unwrap());
    (root, policy)
}

#[test]
fn isolated_shared_container_lifetime_and_legacy_allocation_control() {
    const TEST: &str = "hosted::acp::shared::tests::isolated_shared_container_lifetime_and_legacy_allocation_control";
    if deallocation::isolated(TEST) {
        return;
    }
    let (root, policy) = policy();
    let mut probe = AllocationProbe::new();
    let legacy = probe.track(|| Shared::new([19u8; 1024], None, "legacy fixture").unwrap());
    assert!(probe.live() >= 1024);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
    assert_eq!(legacy.requested_bytes(), 0);
    let clone = legacy.clone();
    drop(legacy);
    assert_eq!(clone[1023], 19);
    drop(clone);
    assert_eq!(probe.live(), 0);
    let (owner, pause) = probe.pause_deallocation_containing(
        deallocation::arc_allocation_bytes::<[u8; 1024]>(),
        || Shared::new([31u8; 1024], Some(&policy), "shared fixture").unwrap(),
        |owner| (&**owner as *const [u8; 1024]) as usize,
    );
    let charged = root.capacity_bytes() - root.available_bytes();
    assert_eq!(owner.requested_bytes(), charged);
    let clone = owner.clone();
    drop(owner);
    assert_eq!(root.capacity_bytes() - root.available_bytes(), charged);
    assert_eq!(clone[1023], 31);
    assert!(
        pause.admission_blocked_while_dropping(&policy, IngressClass::Data, move || drop(clone))
    );
    assert_eq!(probe.live(), 0);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}

#[test]
fn shared_container_pressure_refuses_before_allocating_and_drops_input() {
    let (root, policy) = policy();
    let pressure = policy
        .admit(
            IngressClass::Data,
            AllocationPlan::checked(
                "shared fixture pressure",
                &[policy.budget(IngressClass::Data).available_bytes()],
            )
            .unwrap(),
        )
        .unwrap();
    struct Dropped<'a>(&'a AtomicUsize);
    impl Drop for Dropped<'_> {
        fn drop(&mut self) {
            self.0.fetch_add(1, SeqCst);
        }
    }
    let dropped = AtomicUsize::new(0);
    let mut probe = AllocationProbe::new();
    let error = probe.track(|| {
        Shared::new(Dropped(&dropped), Some(&policy), "refused shared container")
            .err()
            .unwrap()
    });
    assert!(matches!(error, AdmissionRefusal::AtCapacity { .. }));
    assert_eq!(probe.allocated(), 0);
    assert_eq!(dropped.load(SeqCst), 1);
    drop(pressure);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}

#[tokio::test]
async fn bridge_container_clones_keep_storage_after_parent_drop() {
    let fixture = super::super::ingress::fixture::fixture();
    let root = fixture.root.clone();
    let sink = fixture.bridge.sink.clone();
    let calls = fixture.bridge.pending.clone();
    let answers = fixture.bridge.answered.clone();
    let waiters = fixture.bridge.owned_prompt_waiters.clone();
    let outer = sink.requested_bytes()
        + calls.requested_bytes()
        + answers.requested_bytes()
        + waiters.requested_bytes();
    assert!(outer > 0);
    drop(fixture);
    assert!(root.capacity_bytes() - root.available_bytes() >= outer);
    assert!(calls.lock().await.is_empty());
    assert!(answers.lock().await.is_empty());
    waiters.fetch_add(1, SeqCst);
    drop((sink, calls, answers));
    assert_eq!(
        root.capacity_bytes() - root.available_bytes(),
        waiters.requested_bytes()
    );
    assert_eq!(waiters.load(SeqCst), 1);
    drop(waiters);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}

#[test]
fn isolated_bridge_waiter_backing_stays_charged_through_final_deallocation() {
    const TEST: &str = "hosted::acp::shared::tests::isolated_bridge_waiter_backing_stays_charged_through_final_deallocation";
    if deallocation::isolated(TEST) {
        return;
    }
    // JsonPools, permission state and raw output initialize four process
    // singleton semaphores. This probe measures attachment/container teardown.
    drop(super::super::ingress::fixture::fixture());
    let mut probe = AllocationProbe::new();
    let (fixture, pause) = probe.pause_deallocation_containing(
        deallocation::arc_allocation_bytes::<AtomicUsize>(),
        super::super::ingress::fixture::fixture,
        |fixture| (&*fixture.bridge.owned_prompt_waiters as *const AtomicUsize) as usize,
    );
    let root = fixture.root.clone();
    let policy = fixture.policy.clone();
    let first = fixture.bridge.owned_prompt_waiters.clone();
    let last = first.clone();
    let charged = first.requested_bytes();
    drop(fixture);
    assert_eq!(root.capacity_bytes() - root.available_bytes(), charged);
    assert!(
        pause.admission_blocked_while_dropping(&policy, IngressClass::Data, move || {
            let barrier = Arc::new(std::sync::Barrier::new(2));
            let other = barrier.clone();
            let child = std::thread::spawn(move || {
                other.wait();
                drop(first);
            });
            barrier.wait();
            drop(last);
            child.join().unwrap();
        })
    );
    assert_eq!(root.available_bytes(), root.capacity_bytes());
    drop((policy, root));
    assert_eq!(probe.live(), 0);
}
