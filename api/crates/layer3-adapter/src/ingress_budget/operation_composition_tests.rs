use super::*;
use crate::allocation_probe::AllocationProbe;
use crate::ingress_budget::{AdmissionRefusal, ClassBounds, IngressBudget};
use futures_util::FutureExt;
use std::sync::atomic::{AtomicBool, Ordering};

fn plan(bytes: usize) -> AllocationPlan {
    AllocationPlan::checked("composed source fixture", &[bytes]).unwrap()
}

fn fixture() -> (IngressBudget, IngressOwnershipPolicy) {
    let root = IngressBudget::new(1024 * 1024);
    let policy = IngressOwnershipPolicy::new(
        root.clone(),
        ClassBounds::new(root.capacity_bytes(), plan(64 * 1024), plan(64 * 1024)).unwrap(),
    )
    .unwrap();
    drop(policy.admit(IngressClass::Data, plan(1)).unwrap());
    (root, policy)
}

#[test]
fn composed_source_refuses_before_initialization_or_box_allocation() {
    let (root, policy) = fixture();
    let held = policy
        .admit(
            IngressClass::Data,
            plan(policy.budget(IngressClass::Data).available_bytes()),
        )
        .unwrap();
    let called = AtomicBool::new(false);
    let mut probe = AllocationProbe::new();
    for _ in 0..128 {
        let error = probe.track(|| {
            AdmittedOperation::from_owned(&policy, IngressClass::Data, plan(0), |scope| {
                called.store(true, Ordering::Relaxed);
                async move { Ok(scope.initialize_owned(|| ())) }
            })
            .unwrap_err()
        });
        assert!(matches!(
            error.refusal(),
            Some(AdmissionRefusal::AtCapacity { .. })
        ));
    }
    assert!(!called.load(Ordering::Relaxed));
    assert_eq!(probe.allocated(), 0);
    drop(held);
    root.assert_idle();
}

#[test]
fn composed_source_preserves_error_identity_and_rejects_foreign_results() {
    let (root, policy) = fixture();
    let (foreign_root, foreign) = fixture();
    for selected in [&policy, &foreign] {
        let error =
            AdmittedOperation::<()>::new(selected, IngressClass::Data, plan(0), |_| async {
                Err(super::super::diagnostic::make_preserving_category(
                    crate::AdapterError::OperationIdentityConflict,
                    format_args!("synthetic source authority changed"),
                ))
            })
            .unwrap()
            .now_or_never()
            .unwrap()
            .unwrap_err();
        let original = error.clone();
        let returned =
            AdmittedOperation::<()>::from_owned(&policy, IngressClass::Data, plan(0), |_| async {
                Err(error)
            })
            .unwrap()
            .now_or_never()
            .unwrap()
            .unwrap_err();
        if std::ptr::eq(selected, &policy) {
            assert!(std::ptr::eq(
                returned.as_adapter().unwrap(),
                original.as_adapter().unwrap()
            ));
        } else {
            assert!(matches!(
                returned.refusal(),
                Some(AdmissionRefusal::InvalidPlan { .. })
            ));
        }
        drop((returned, original));
        root.assert_idle();
        foreign_root.assert_idle();
    }
    let returned = AdmittedOperation::from_owned(&policy, IngressClass::Data, plan(0), |_| async {
        AdmittedOperation::new(&foreign, IngressClass::Data, plan(8192), |_| async {
            Ok(vec![1u8; 8192])
        })?
        .await
    })
    .unwrap()
    .now_or_never()
    .unwrap()
    .unwrap_err();
    assert!(matches!(
        returned.refusal(),
        Some(AdmissionRefusal::InvalidPlan { .. })
    ));
    root.assert_idle();
    foreign_root.assert_idle();
}

#[test]
fn isolated_composed_result_and_future_have_independent_physical_lifetimes() {
    const TEST: &str = "ingress_budget::operation::composition_tests::isolated_composed_result_and_future_have_independent_physical_lifetimes";
    if std::env::var_os("CMA_COMPOSED_SOURCE_CHILD").is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", TEST, "--nocapture", "--test-threads=1"])
            .env("CMA_COMPOSED_SOURCE_CHILD", "1")
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
    for result_first in [false, true] {
        let (root, policy) = fixture();
        let mut probe = AllocationProbe::new();
        let mut operation = probe.track(|| {
            AdmittedOperation::from_owned(&policy, IngressClass::Data, plan(4096), |_| async {
                let temporary = vec![1u8; 4096];
                let source =
                    AdmittedOperation::new(&policy, IngressClass::Data, plan(65_536), |_| async {
                        Ok(vec![9u8; 65_536])
                    })?;
                let value = source.await?;
                assert_eq!(temporary.len(), 4096);
                Ok(value)
            })
            .unwrap()
        });
        let result = probe.track(|| (&mut operation).now_or_never().unwrap().unwrap());
        let charged = root.reserved_work_bytes();
        assert!(probe.peak() <= charged);
        assert_eq!(result.len(), 65_536);
        eprintln!(
            "composed_source peak={} retained={} admitted={charged}",
            probe.peak(),
            probe.live()
        );
        if result_first {
            drop(result);
            assert!(probe.live() > 0);
            assert_eq!(root.reserved_work_bytes(), operation.requested_bytes());
            drop(operation);
        } else {
            drop(operation);
            assert!(probe.live() >= 65_536);
            assert!(root.reserved_work_bytes() >= probe.live());
            drop(result);
        }
        assert_eq!(probe.live(), 0);
        root.assert_idle();
    }
}
