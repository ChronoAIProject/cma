use super::runtime_output_ownership_tests::{ack_policy, source};
use super::*;
use crate::allocation_probe::{deallocation, AllocationProbe};
use crate::ingress_budget::AdmissionRefusal;
use futures_util::FutureExt;

fn fixture() -> (
    crate::ingress_budget::IngressBudget,
    IngressOwnershipPolicy,
    RuntimeOutputReceipt,
    AdmittedAckReceiver,
) {
    let (root, policy) = ack_policy();
    let (sender, receiver) =
        RuntimeOutputReceipt::admitted_channel_checked(policy.clone(), 1).unwrap();
    (
        root,
        policy,
        RuntimeOutputReceipt::new_admitted(source(), sender),
        receiver,
    )
}

fn repeat(receipt: &RuntimeOutputReceipt, expected: AdmittedAckError) {
    let mut probe = AllocationProbe::new();
    let errors: [_; 128] =
        std::array::from_fn(|_| probe.track(|| receipt.acknowledge_admitted().unwrap_err()));
    assert!(errors.iter().all(|error| *error == expected));
    assert_eq!(probe.allocated(), 0);
    assert_eq!(probe.live(), 0);
}

#[test]
fn isolated_full_and_closed_errors_can_outlive_the_operation_without_allocating() {
    if deallocation::isolated("runtime_output::ack_error_tests::isolated_full_and_closed_errors_can_outlive_the_operation_without_allocating") { return; }
    let (root, _, receipt, receiver) = fixture();
    receipt.acknowledge_admitted().unwrap();
    let baseline = root.available_bytes();
    let mut probe = AllocationProbe::new();
    // The compatibility API demonstrates the original retained-error gap.
    let errors: [_; 128] = std::array::from_fn(|_| {
        probe.track(|| receipt.acknowledge().now_or_never().unwrap().unwrap_err())
    });
    assert!(errors
        .iter()
        .all(|error| matches!(error, crate::AdapterError::AtCapacity(_))));
    assert!(probe.live() >= 128 * "runtime acknowledgement queue is full".len());
    assert_eq!(root.available_bytes(), baseline);
    drop(errors);
    assert_eq!(probe.live(), 0);
    drop(probe);
    repeat(&receipt, AdmittedAckError::Full);
    assert_eq!(root.available_bytes(), baseline);
    drop(receiver);
    repeat(&receipt, AdmittedAckError::Closed);
    drop(receipt);
    root.assert_idle();
}

#[test]
fn invalid_and_poisoned_ack_errors_do_not_copy_the_identity() {
    let (root, _, mut receipt, receiver) = fixture();
    let baseline = root.available_bytes();
    receipt.source.native_conversation_id = "x".repeat(4096);
    let original = receipt.source.native_conversation_id.as_ptr();
    repeat(&receipt, AdmittedAckError::IdentityTooLarge);
    assert_eq!(receipt.source.native_conversation_id.as_ptr(), original);
    receipt.source.native_conversation_id.truncate(1024);
    let Acknowledgements::Admitted(sender) = &receipt.acknowledgements else {
        unreachable!()
    };
    let poisoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _lock = sender.queue.enqueue.lock().unwrap();
        panic!("synthetic ACK queue poison");
    }));
    assert!(poisoned.is_err());
    repeat(&receipt, AdmittedAckError::Poisoned);
    assert_eq!(root.available_bytes(), baseline);
    drop((receipt, receiver));
    root.assert_idle();
}

#[test]
fn control_pressure_preserves_original_receipt_and_allows_exact_retry() {
    let (root, policy, receipt, mut receiver) = fixture();
    let before = receipt.source.clone();
    let available = policy.budget(IngressClass::Control).available_bytes();
    let pressure = policy
        .admit(
            IngressClass::Control,
            AllocationPlan::checked("ACK control pressure", &[available]).unwrap(),
        )
        .unwrap()
        .initialize(|| vec![0_u8; available - 512]);
    repeat(
        &receipt,
        AdmittedAckError::Admission(AdmissionRefusal::AtCapacity {
            phase: "acknowledgement identity",
        }),
    );
    assert_eq!(receipt.source, before);
    assert!(receiver.receiver.try_recv().is_err());
    drop(pressure);
    receipt.acknowledge_admitted().unwrap();
    let ack = receiver.recv().now_or_never().unwrap().unwrap();
    assert_eq!(*ack, before);
    assert_ne!(
        ack.native_conversation_id.as_ptr(),
        receipt.source.native_conversation_id.as_ptr()
    );
    drop((receipt, receiver));
    assert!(root.available_bytes() < root.capacity_bytes());
    drop(ack);
    root.assert_idle();
}

#[test]
fn checked_channel_refuses_invalid_or_exhausted_capacity_without_allocating() {
    let (root, policy) = ack_policy();
    let mut probe = AllocationProbe::new();
    for capacity in [0, usize::MAX] {
        let error = probe.track(|| {
            RuntimeOutputReceipt::admitted_channel_checked(policy.clone(), capacity)
                .err()
                .unwrap()
        });
        assert!(matches!(
            error,
            AdmittedAckError::InvalidCapacity
                | AdmittedAckError::Admission(AdmissionRefusal::InvalidPlan { .. })
        ));
    }
    assert_eq!(probe.allocated(), 0);
    root.assert_idle();
    drop(probe);
    let pressure = policy
        .admit(
            IngressClass::Control,
            AllocationPlan::checked("ACK channel pressure", &[policy.bounds().control_bytes()])
                .unwrap(),
        )
        .unwrap();
    let mut probe = AllocationProbe::new();
    let errors: [_; 128] = std::array::from_fn(|_| {
        probe.track(|| {
            RuntimeOutputReceipt::admitted_channel_checked(policy.clone(), 1)
                .err()
                .unwrap()
        })
    });
    assert!(errors.iter().all(|error| matches!(
        error,
        AdmittedAckError::Admission(AdmissionRefusal::AtCapacity { .. })
    )));
    assert_eq!(probe.allocated(), 0);
    drop((probe, pressure));
    root.assert_idle();
}

#[test]
fn typed_ack_never_falls_back_to_the_legacy_queue() {
    let (sender, mut receiver) = mpsc::channel(1);
    let receipt = RuntimeOutputReceipt::new(source(), sender);
    repeat(&receipt, AdmittedAckError::OwnershipRequired);
    assert!(receiver.try_recv().is_err());
    receipt.acknowledge().now_or_never().unwrap().unwrap();
    assert_eq!(receiver.try_recv().unwrap(), receipt.source);
}

#[test]
fn compatibility_ack_keeps_the_existing_failure_categories_and_messages() {
    let (_, _, mut receipt, mut receiver) = fixture();
    receipt.acknowledge().now_or_never().unwrap().unwrap();
    assert_eq!(
        receipt.acknowledge().now_or_never().unwrap(),
        Err(crate::AdapterError::AtCapacity(
            "runtime acknowledgement queue is full".into()
        ))
    );
    drop(receiver.recv().now_or_never().unwrap().unwrap());
    receipt.source.stream_id.push('x');
    assert_eq!(
        receipt.acknowledge().now_or_never().unwrap(),
        Err(crate::AdapterError::Rejected(
            "runtime acknowledgement identity exceeds its bound".into()
        ))
    );
    receipt.source.stream_id.pop();
    drop(receiver);
    assert_eq!(
        receipt.acknowledge().now_or_never().unwrap(),
        Err(crate::AdapterError::Unavailable(
            "runtime acknowledgement link ended".into()
        ))
    );
}
