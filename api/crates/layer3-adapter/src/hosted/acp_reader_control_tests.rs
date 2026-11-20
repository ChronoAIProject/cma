use super::*;
use crate::allocation_probe::AllocationProbe;
use crate::ingress_budget::{sizing, AllocationPlan, IngressClass};
use futures_util::{future::poll_fn, FutureExt};
use std::{cell::Cell, task::Poll};

#[test]
fn reader_and_ack_diagnostics_fit_beside_declared_control_work() {
    let fixture = super::super::ingress::fixture::fixture();
    let policy = &fixture.policy;
    let attachments = policy.bounds().maximum_attachments().unwrap();
    let control = policy.budget(IngressClass::Control);
    for class in [IngressClass::Data, IngressClass::Control] {
        drop(
            policy
                .admit(
                    class,
                    AllocationPlan::checked("resident lock", &[1]).unwrap(),
                )
                .unwrap(),
        );
    }

    // Materialize the other declared control phases at their admitted peak:
    // the ACK queue/copies/write, two permission replies, and the input ID copy.
    // The diagnostic wrappers must also fit for both tasks on every attachment.
    let reply = crate::hosted::permission_state::reply_plan()
        .unwrap()
        .requested_bytes()
        + 4097
        + sizing::shared_owner_bytes::<Message>();
    let other = crate::RuntimeOutputReceipt::control_bound(128)
        .unwrap()
        .requested_bytes()
        + 2 * reply
        + 1024;
    let resident = policy.bounds().control_bytes() - control.available_bytes();
    let pressure = |class, bytes| {
        policy
            .admit(
                class,
                AllocationPlan::checked("simultaneous declared work", &[bytes]).unwrap(),
            )
            .unwrap()
            .initialize(|| vec![0u8; bytes - sizing::shared_owner_bytes::<Vec<u8>>()])
    };
    let other_work = pressure(IngressClass::Control, attachments * other - resident);
    let data = pressure(
        IngressClass::Data,
        policy.budget(IngressClass::Data).available_bytes(),
    );
    assert_eq!(policy.budget(IngressClass::Data).available_bytes(), 0);

    let entered = Cell::new(0usize);
    let release = Cell::new(false);
    let mut operations: Vec<_> = (0..2 * attachments)
        .map(|_| {
            Box::pin(with_diagnostics(&fixture.bridge, async {
                entered.set(entered.get() + 1);
                poll_fn(|_| {
                    if release.get() {
                        Poll::Ready(true)
                    } else {
                        Poll::Pending
                    }
                })
                .await
            }))
        })
        .collect();
    let before = control.available_bytes();
    let mut probe = AllocationProbe::new();
    probe.track(|| {
        for operation in &mut operations {
            assert!(
                operation.as_mut().now_or_never().is_none(),
                "Control floor refused reader/ACK diagnostics beside declared work"
            );
        }
    });
    assert_eq!(entered.get(), 2 * attachments);
    let charged = before - control.available_bytes();
    assert!(probe.allocated() > 0);
    assert!(
        probe.peak() <= charged,
        "diagnostic peak exceeds its custody"
    );
    release.set(true);
    probe.track(|| {
        for operation in &mut operations {
            assert_eq!(operation.as_mut().now_or_never(), Some(true));
        }
    });
    drop(operations);
    assert_eq!(probe.live(), 0);
    assert_eq!(control.available_bytes(), before);
    drop((data, other_work));
}
