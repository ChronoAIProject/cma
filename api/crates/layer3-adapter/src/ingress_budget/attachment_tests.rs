use super::*;
use crate::allocation_probe::{deallocation, AllocationProbe};
use crate::ingress_budget::{sizing, ClassBounds, IngressReservation, IngressWorkScope};
use std::sync::{Arc, Barrier};

fn policy(maximum: usize) -> (IngressBudget, IngressOwnershipPolicy) {
    let root = IngressBudget::new(64 * 1024);
    let floor = AllocationPlan::checked("attachment test floor", &[8192]).unwrap();
    let bounds = ClassBounds::new(root.capacity_bytes(), floor, floor)
        .unwrap()
        .with_attachment_limit(maximum)
        .unwrap();
    let policy = IngressOwnershipPolicy::new(root.clone(), bounds).unwrap();
    drop(policy.admit(IngressClass::Data, floor).unwrap());
    (root, policy)
}

#[test]
fn repeated_policies_share_the_limit_and_refuse_before_allocation() {
    let (root, policy) = policy(1);
    let repeated = IngressOwnershipPolicy::new(root.clone(), policy.bounds()).unwrap();
    let attachment = policy.admit_attachment().unwrap();
    let retained = attachment.policy().clone();
    let mut probe = AllocationProbe::new();
    let refusal = probe.track(|| repeated.admit_attachment());
    assert!(matches!(refusal, Err(AdmissionRefusal::AtCapacity { .. })));
    assert!(matches!(
        retained.admit_attachment(),
        Err(AdmissionRefusal::InvalidPlan { .. })
    ));
    assert_eq!(probe.allocated(), 0);
    drop(attachment);
    assert!(policy.admit_attachment().is_err());
    drop(retained);
    drop(repeated.admit_attachment().unwrap());
    assert_eq!(root.available_bytes(), root.capacity_bytes());
    let different = policy.bounds().with_attachment_limit(2).unwrap();
    assert!(IngressOwnershipPolicy::new(root.clone(), different).is_err());
}

#[test]
fn capacity_refusal_rolls_back_the_slot_without_allocating() {
    let (root, policy) = policy(1);
    let pressure = policy
        .admit(
            IngressClass::Data,
            AllocationPlan::checked("attachment pressure", &[policy.bounds().data_bytes()])
                .unwrap(),
        )
        .unwrap();
    let mut probe = AllocationProbe::new();
    for _ in 0..2 {
        let refusal = probe.track(|| policy.admit_attachment());
        assert!(matches!(
            refusal,
            Err(AdmissionRefusal::AtCapacity {
                phase: "attachment lifetime owner"
            })
        ));
    }
    assert_eq!(probe.allocated(), 0);
    drop(pressure);
    drop(policy.admit_attachment().unwrap());
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}

#[test]
fn policies_without_a_counted_attachment_bound_cannot_start_one() {
    let root = IngressBudget::new(64 * 1024);
    let floor = AllocationPlan::checked("unbound attachment floor", &[8192]).unwrap();
    let policy = IngressOwnershipPolicy::new(
        root.clone(),
        ClassBounds::new(root.capacity_bytes(), floor, floor).unwrap(),
    )
    .unwrap();
    let mut probe = AllocationProbe::new();
    assert!(matches!(
        probe.track(|| policy.admit_attachment()),
        Err(AdmissionRefusal::InvalidPlan { .. })
    ));
    assert_eq!(probe.allocated(), 0);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}

#[test]
fn split_payloads_and_component_reservations_retain_the_attachment() {
    let (root, policy) = policy(1);
    let attachment = policy.admit_attachment().unwrap();
    let data = attachment.policy().budget(IngressClass::Data);
    let plan = AllocationPlan::checked(
        "retained attachment payload",
        &[1024 + sizing::shared_owner_bytes::<Vec<u8>>()],
    )
    .unwrap();
    let mut storage = attachment.policy().admit(IngressClass::Data, plan).unwrap();
    let payload = storage
        .split_admitted(plan)
        .unwrap()
        .initialize(|| vec![11u8; 1024]);
    let component = Arc::new(tokio::sync::Semaphore::new(256));
    let component_storage = IngressReservation::try_acquire(&component, Some(&data), 128).unwrap();
    drop((attachment, data, storage));
    assert!(policy.admit_attachment().is_err());
    let last = payload.clone();
    drop(payload);
    assert_eq!(last[1023], 11);
    drop(last);
    assert!(policy.admit_attachment().is_err());
    drop(component_storage);
    drop(policy.admit_attachment().unwrap());
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}

#[test]
fn scope_extensions_keep_their_own_attachment_through_unwind() {
    let (root, policy) = policy(1);
    let attachment = policy.admit_attachment().unwrap();
    let scope = IngressWorkScope::new(
        attachment
            .policy()
            .admit(
                IngressClass::Data,
                AllocationPlan::checked("parent scope", &[sizing::shared_owner_bytes::<()>()])
                    .unwrap(),
            )
            .unwrap(),
    );
    scope
        .admit_extra(
            attachment.policy(),
            IngressClass::Data,
            AllocationPlan::checked("attached scope extension", &[1024]).unwrap(),
        )
        .unwrap();
    let payload = scope.initialize_owned(|| vec![3u8; 1024]);
    drop((scope, attachment));
    assert!(policy.admit_attachment().is_err());
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        let _payload = payload;
        panic!("synthetic attachment unwind");
    }));
    assert!(result.is_err());
    drop(policy.admit_attachment().unwrap());
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}

#[test]
fn retention_transfers_the_attachment_and_rejects_another_attachments_owner() {
    let (root, policy) = policy(2);
    let first = policy.admit_attachment().unwrap();
    let second = policy.admit_attachment().unwrap();
    let plan = AllocationPlan::checked(
        "attachment retention transfer",
        &[2 * (1024 + sizing::shared_owner_bytes::<Vec<u8>>())],
    )
    .unwrap();
    let scope = IngressWorkScope::new(first.policy().admit(IngressClass::Data, plan).unwrap());
    let value = scope.initialize_owned(|| vec![17u8; 1024]);
    assert!(value.belongs_to(first.policy()));
    assert!(value.belongs_to(&policy));
    assert!(!value.belongs_to(second.policy()));
    assert!(scope
        .admit_extra(second.policy(), IngressClass::Data, plan)
        .is_err());
    let invalid = scope.initialize_owned(|| vec![19u8; 1024]);
    assert!(invalid
        .retain_admitted(second.policy().admit(IngressClass::Data, plan).unwrap())
        .is_err());
    let retained = value
        .retain_admitted(policy.admit(IngressClass::Data, plan).unwrap())
        .unwrap();
    assert!(retained.belongs_to(first.policy()));
    drop((scope, first));
    assert!(policy.admit_attachment().is_err());
    assert_eq!(retained[1023], 17);
    drop(retained);
    drop(policy.admit_attachment().unwrap());
    drop(second);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}

#[test]
fn concurrent_admission_counts_retained_payloads_after_startup_tokens_drop() {
    let (root, policy) = policy(4);
    let barrier = Arc::new(Barrier::new(17));
    let mut workers = Vec::new();
    for _ in 0..16 {
        let barrier = barrier.clone();
        let policy = policy.clone();
        workers.push(std::thread::spawn(move || {
            let payload = policy.admit_attachment().ok().map(|attachment| {
                attachment
                    .policy()
                    .admit(
                        IngressClass::Data,
                        AllocationPlan::checked(
                            "concurrent attachment payload",
                            &[128 + sizing::shared_owner_bytes::<Vec<u8>>()],
                        )
                        .unwrap(),
                    )
                    .unwrap()
                    .initialize(|| vec![13u8; 128])
            });
            barrier.wait();
            barrier.wait();
            payload
        }));
    }
    barrier.wait();
    assert!(policy.admit_attachment().is_err());
    barrier.wait();
    let payloads: Vec<_> = workers
        .into_iter()
        .filter_map(|worker| worker.join().unwrap())
        .collect();
    assert_eq!(payloads.len(), 4);
    assert!(payloads.iter().all(|payload| payload[127] == 13));
    assert!(policy.admit_attachment().is_err());
    drop(payloads);
    drop(policy.admit_attachment().unwrap());
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}

#[test]
fn isolated_attachment_slot_and_storage_survive_physical_owner_deallocation() {
    const TEST: &str = "ingress_budget::attachment::tests::isolated_attachment_slot_and_storage_survive_physical_owner_deallocation";
    if deallocation::isolated(TEST) {
        return;
    }
    let (root, policy) = policy(1);
    let mut probe = AllocationProbe::new();
    let (attachment, pause) = probe.pause_deallocation(
        deallocation::arc_allocation_bytes::<AttachmentStorage>(),
        || policy.admit_attachment().unwrap(),
    );
    let charged = root.capacity_bytes() - root.available_bytes();
    let retained = attachment.policy().clone();
    drop(attachment);
    pause.inspect_while_dropping(
        move || drop(retained),
        || {
            assert!(policy.admit_attachment().is_err());
            assert_eq!(root.capacity_bytes() - root.available_bytes(), charged);
        },
    );
    assert!(probe.allocated() <= charged);
    assert_eq!(probe.live(), 0);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
    drop(policy.admit_attachment().unwrap());
}

#[test]
fn isolated_inside_arc_slot_control_returns_before_physical_deallocation() {
    const TEST: &str = "ingress_budget::attachment::tests::isolated_inside_arc_slot_control_returns_before_physical_deallocation";
    if deallocation::isolated(TEST) {
        return;
    }
    let (root, policy) = policy(1);
    let mut probe = AllocationProbe::new();
    let (owner, pause) = probe
        .pause_deallocation(deallocation::arc_allocation_bytes::<Slot>(), || {
            Arc::new(Slot::acquire(root.clone(), 1).unwrap())
        });
    assert!(policy.admit_attachment().is_err());
    pause.inspect_while_dropping(
        move || drop(owner),
        || {
            let _replacement = policy.admit_attachment().unwrap();
        },
    );
    assert_eq!(probe.live(), 0);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}
