use super::tests::fixture;
use super::*;
use crate::allocation_probe::AllocationProbe;
use crate::ingress_budget::sizing;
use futures_util::FutureExt;

fn source_error(policy: &IngressOwnershipPolicy) -> AdmittedError {
    policy
        .admit(
            IngressClass::Data,
            AllocationPlan::checked(
                "source error fixture",
                &[4096, sizing::shared_owner_bytes::<AdapterError>()],
            )
            .unwrap(),
        )
        .unwrap()
        .initialize(|| AdapterError::RuntimeLedgerHold("s".repeat(4096)))
        .into()
}

#[test]
fn diagnostic_source_result_preserves_owner_identity_and_last_clone() {
    if crate::allocation_probe::deallocation::isolated("ingress_budget::diagnostic::source_tests::diagnostic_source_result_preserves_owner_identity_and_last_clone") { return; }
    let (root, policy, scope) = fixture();
    let mut probe = AllocationProbe::new();
    let error = probe.track(|| source_error(&policy));
    let original = error.as_adapter().unwrap() as *const AdapterError;
    let source_bytes = error.requested_bytes();
    let result = probe.track(|| {
        scope
            .initialize_owned(|| async move { Err::<(), _>(OperationError::Source(error)) })
            .resolve_source_result(&policy, IngressClass::Data)
            .now_or_never()
            .unwrap()
            .unwrap_err()
    });
    assert_eq!(
        result.as_adapter().unwrap() as *const AdapterError,
        original
    );
    assert_eq!(result.requested_bytes(), source_bytes);
    let copy = probe.track(|| result.clone());
    drop((scope, result));
    assert!(probe.live() >= 4096);
    assert!(probe.live() <= copy.requested_bytes());
    assert_eq!(root.capacity_bytes() - root.available_bytes(), source_bytes);
    drop(copy);
    assert_eq!(probe.live(), 0);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}

#[test]
fn diagnostic_source_result_keeps_source_when_local_capacity_is_full() {
    let (root, policy, scope) = fixture();
    let error = source_error(&policy);
    let source_bytes = error.requested_bytes();
    let held_pressure = std::cell::RefCell::new(None);
    let mut probe = AllocationProbe::new();
    let result = scope
        .initialize_owned(|| async {
            let pressure = policy
                .admit(
                    IngressClass::Data,
                    AllocationPlan::checked(
                        "source error pressure",
                        &[policy.budget(IngressClass::Data).available_bytes()],
                    )
                    .unwrap(),
                )
                .unwrap();
            let result = probe.track(|| Err::<(), _>(OperationError::Source(error)));
            *held_pressure.borrow_mut() = Some(pressure);
            result
        })
        .resolve_source_result(&policy, IngressClass::Data)
        .now_or_never()
        .unwrap()
        .unwrap_err();
    assert_eq!(probe.allocated(), 0);
    assert_eq!(policy.budget(IngressClass::Data).available_bytes(), 0);
    drop(held_pressure);
    assert!(
        matches!(result.as_adapter(), Some(AdapterError::RuntimeLedgerHold(detail)) if detail.len() == 4096)
    );
    drop(scope);
    assert_eq!(root.capacity_bytes() - root.available_bytes(), source_bytes);
    drop(result);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}

#[test]
fn diagnostic_source_result_retains_local_errors_and_rejects_foreign_owners() {
    let (root, policy, scope) = fixture();
    let local = scope
        .initialize_owned(|| async {
            Err::<(), _>(OperationError::Local(make_preserving_category(
                AdapterError::OperationIdentityConflict,
                format_args!("exact local failure"),
            )))
        })
        .resolve_source_result(&policy, IngressClass::Data)
        .now_or_never()
        .unwrap()
        .unwrap_err();
    assert!(
        matches!(local.as_adapter(), Some(AdapterError::OperationIdentityConflict(detail)) if detail == "exact local failure")
    );
    assert_eq!(local.requested_bytes(), scope.requested_bytes());
    drop(local);
    let (foreign_root, foreign, foreign_scope) = fixture();
    let error = source_error(&foreign);
    let result = scope
        .initialize_owned(|| async { Err::<(), _>(OperationError::Source(error)) })
        .resolve_source_result(&policy, IngressClass::Data)
        .now_or_never()
        .unwrap()
        .unwrap_err();
    assert_eq!(
        result.refusal(),
        Some(AdmissionRefusal::InvalidPlan {
            phase: "diagnostic source error ownership"
        })
    );
    drop((scope, foreign_scope));
    assert_eq!(root.available_bytes(), root.capacity_bytes());
    assert_eq!(
        foreign_root.available_bytes(),
        foreign_root.capacity_bytes()
    );
}

#[test]
fn diagnostic_source_cancellation_drops_both_source_and_local_storage() {
    let (root, policy, scope) = fixture();
    let source = source_error(&policy);
    let future = scope.initialize_owned(|| async move {
        let local =
            make_preserving_category(AdapterError::Unavailable, format_args!("local scratch"));
        std::future::pending::<()>().await;
        drop(local);
        Err::<(), _>(OperationError::Source(source))
    });
    let mut pending = Box::pin(future.resolve_source_result(&policy, IngressClass::Data));
    assert!(pending.as_mut().now_or_never().is_none());
    drop(scope);
    assert!(root.available_bytes() < root.capacity_bytes());
    drop(pending);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}
