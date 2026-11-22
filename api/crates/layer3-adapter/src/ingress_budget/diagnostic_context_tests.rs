use super::*;
use crate::allocation_probe::AllocationProbe;
use crate::ingress_budget::{sizing, ClassBounds, IngressBudget};
use futures_util::FutureExt;

fn fixture() -> (IngressBudget, IngressOwnershipPolicy, IngressWorkScope) {
    let root = IngressBudget::new(1024 * 1024);
    let floor = AllocationPlan::checked("context floor", &[64 * 1024]).unwrap();
    let policy = IngressOwnershipPolicy::new(
        root.clone(),
        ClassBounds::new(root.capacity_bytes(), floor, floor).unwrap(),
    )
    .unwrap();
    for class in [
        IngressClass::Data,
        IngressClass::Transformation,
        IngressClass::Control,
    ] {
        drop(
            policy
                .admit(
                    class,
                    AllocationPlan::checked("warm shared pool", &[1]).unwrap(),
                )
                .unwrap(),
        );
    }
    let scope = IngressWorkScope::new(
        policy
            .admit(
                IngressClass::Data,
                AllocationPlan::checked("context scope", &[sizing::shared_owner_bytes::<()>()])
                    .unwrap(),
            )
            .unwrap(),
    );
    (root, policy, scope)
}

fn categories() -> [fn(String) -> AdapterError; 8] {
    [
        AdapterError::PromptNotWritten,
        AdapterError::InputAuthorityUnavailable,
        AdapterError::Unavailable,
        AdapterError::Rejected,
        AdapterError::OperationIdentityConflict,
        AdapterError::RuntimeLedgerHold,
        AdapterError::AtCapacity,
        AdapterError::Unsupported,
    ]
}

#[test]
fn isolated_error_context_admits_old_and_new_text_overlap() {
    const TEST: &str = "ingress_budget::diagnostic::context_tests::isolated_error_context_admits_old_and_new_text_overlap";
    if std::env::var_os("CMA_CONTEXT_ALLOCATION_CHILD").is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", TEST, "--nocapture", "--test-threads=1"])
            .env("CMA_CONTEXT_ALLOCATION_CHILD", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
        eprintln!("{}", String::from_utf8_lossy(&output.stderr));
        return;
    }
    let source = "x".repeat(32 * 1024);
    let context = "y".repeat(64 * 1024);
    let expected = format!("{context}: {source}");
    for category in categories() {
        let (root, policy, scope) = fixture();
        let mut probe = AllocationProbe::new();
        let error = probe.track(|| {
            scope
                .initialize_owned(|| async {
                    Err::<(), _>(make(category, format_args!("{source}")).context(&context))
                })
                .resolve_admitted_result(&policy, IngressClass::Data)
                .now_or_never()
                .unwrap()
                .unwrap_err()
        });
        let charged = scope.requested_bytes();
        assert!(
            probe.peak() <= charged,
            "context peak={} admission={charged}",
            probe.peak()
        );
        assert_eq!(error.as_adapter(), Some(&category(expected.clone())));
        let clone = probe.track(|| error.clone());
        drop((scope, error));
        assert!(probe.live() >= expected.len());
        assert_eq!(root.reserved_work_bytes(), charged);
        eprintln!(
            "context peak={} retained={} admitted={charged}",
            probe.peak(),
            probe.live()
        );
        probe.track(|| drop(clone));
        assert_eq!(probe.live(), 0);
        root.assert_idle();
    }
    let mut probe = AllocationProbe::new();
    let error = probe.track(|| AdapterError::Unavailable(source.clone()).context(&context));
    assert!(
        probe.peak() > expected.len(),
        "legacy control must expose overlapping text"
    );
    drop(error);
    assert_eq!(probe.live(), 0);
}

#[test]
fn context_pressure_preserves_the_original_runtime_category_and_detail() {
    for category in categories() {
        let (root, policy, scope) = fixture();
        let mut probe = AllocationProbe::new();
        let error = scope
            .initialize_owned(|| async {
                let original = make(category, format_args!("original failure"));
                let available = policy.budget(IngressClass::Data).available_bytes();
                let _pressure = policy
                    .admit(
                        IngressClass::Data,
                        AllocationPlan::checked("context pressure", &[available]).unwrap(),
                    )
                    .unwrap()
                    .initialize(|| vec![0u8; available - 1024]);
                Err::<(), _>(probe.track(|| original.context("additional context")))
            })
            .resolve_admitted_result(&policy, IngressClass::Data)
            .now_or_never()
            .unwrap()
            .unwrap_err();
        assert_eq!(probe.allocated(), 0, "refuse before allocating context");
        assert_eq!(
            error.as_adapter(),
            Some(&category("original failure".into()))
        );
        drop((scope, error));
        root.assert_idle();
    }
}

#[test]
fn legacy_context_keeps_exact_text_and_typed_errors() {
    for category in categories() {
        assert_eq!(
            category("detail".into()).context("operation"),
            category("operation: detail".into())
        );
    }
    let typed = AdapterError::RuntimeContractMismatch(crate::RuntimeContractMismatch {
        image: "synthetic-image".into(),
        expected: "3".into(),
        found: Some("2".into()),
    });
    assert_eq!(typed.clone().context("operation"), typed);
}
