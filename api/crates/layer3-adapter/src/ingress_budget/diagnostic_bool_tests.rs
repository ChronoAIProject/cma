use super::*;
use crate::allocation_probe::AllocationProbe;
use crate::ingress_budget::{sizing, ClassBounds, IngressBudget};
use futures_util::{future::poll_fn, FutureExt};
use std::{cell::Cell, task::Poll};

fn fixture() -> (IngressBudget, IngressOwnershipPolicy, IngressWorkScope) {
    let root = IngressBudget::new(1024 * 1024);
    let floor = AllocationPlan::checked("boolean diagnostic floor", &[64 * 1024]).unwrap();
    let policy = IngressOwnershipPolicy::new_admitted(
        root.clone(),
        ClassBounds::new(root.capacity_bytes(), floor, floor).unwrap(),
    )
    .unwrap();
    drop(
        policy
            .admit(
                IngressClass::Control,
                AllocationPlan::checked("resident lock", &[1]).unwrap(),
            )
            .unwrap(),
    );
    let scope = IngressWorkScope::new(
        policy
            .admit(IngressClass::Control, scope_entry_plan().unwrap())
            .unwrap(),
    );
    (root, policy, scope)
}

#[test]
fn boolean_diagnostics_hold_formatted_errors_until_completion_or_cancellation() {
    for cancel in [false, true] {
        let (root, policy, scope) = fixture();
        let release = Cell::new(false);
        let detail = "x".repeat(8192);
        let mut operation = Box::pin(
            scope
                .initialize_owned(|| async {
                    let error = make(AdapterError::Unavailable, format_args!("{detail}"));
                    assert!(matches!(&error, AdapterError::Unavailable(text) if text == &detail));
                    poll_fn(|_| {
                        if release.get() {
                            Poll::Ready(())
                        } else {
                            Poll::Pending
                        }
                    })
                    .await;
                    drop(error);
                    false
                })
                .resolve_diagnostic_bool(&policy, IngressClass::Control),
        );
        let mut probe = AllocationProbe::new();
        assert!(probe.track(|| operation.as_mut().now_or_never()).is_none());
        assert!(probe.live() >= detail.len());
        assert!(probe.peak() <= scope.requested_bytes());
        let held = root.available_bytes();
        drop(scope);
        assert_eq!(root.available_bytes(), held);
        if !cancel {
            release.set(true);
            assert_eq!(
                probe.track(|| operation.as_mut().now_or_never()),
                Some(Ok(false))
            );
        }
        probe.track(|| drop(operation));
        assert_eq!(probe.live(), 0);
        root.assert_idle();
    }
}

#[test]
fn boolean_diagnostics_preserve_fixed_outcomes_when_text_cannot_be_admitted() {
    let (root, policy, scope) = fixture();
    let bytes = policy.budget(IngressClass::Control).available_bytes();
    let pressure = policy
        .admit(
            IngressClass::Control,
            AllocationPlan::checked("diagnostic text pressure", &[bytes]).unwrap(),
        )
        .unwrap()
        .initialize(|| vec![0u8; bytes - sizing::shared_owner_bytes::<Vec<u8>>()]);
    let mut probe = AllocationProbe::new();
    let result = probe.track(|| {
        scope
            .initialize_owned(|| async {
                matches!(
                    make_preserving_category(
                        AdapterError::RuntimeLedgerHold,
                        format_args!("diagnostic text")
                    ),
                    AdapterError::RuntimeLedgerHold(text) if text.is_empty()
                )
            })
            .resolve_diagnostic_bool(&policy, IngressClass::Control)
            .now_or_never()
            .unwrap()
    });
    assert_eq!(result, Ok(true));
    assert_eq!(probe.allocated(), 0);
    drop((scope, pressure));
    root.assert_idle();
}

#[test]
fn boolean_diagnostics_reject_foreign_custody_before_polling() {
    let (root, policy, scope) = fixture();
    let (other_root, other_policy, other_scope) = fixture();
    let entered = Cell::new(false);
    let mut probe = AllocationProbe::new();
    let result = probe.track(|| {
        scope
            .initialize_owned(|| async {
                entered.set(true);
                true
            })
            .resolve_diagnostic_bool(&other_policy, IngressClass::Control)
            .now_or_never()
            .unwrap()
    });
    assert!(matches!(result, Err(AdmissionRefusal::InvalidPlan { .. })));
    assert!(!entered.get());
    assert_eq!(probe.allocated(), 0);
    drop((scope, other_scope, policy, other_policy));
    root.assert_idle();
    other_root.assert_idle();
}
