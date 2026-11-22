use super::*;
use crate::allocation_probe::AllocationProbe;
use crate::ingress_budget::{AdmissionRefusal, ClassBounds, IngressBudget};
use futures_util::FutureExt;
use std::sync::atomic::{AtomicBool, Ordering};

fn plan(bytes: usize) -> AllocationPlan {
    AllocationPlan::checked("source fixture", &[bytes]).unwrap()
}

fn fixture() -> (IngressBudget, IngressOwnershipPolicy) {
    let root = IngressBudget::new(1024 * 1024);
    let policy = IngressOwnershipPolicy::new(
        root.clone(),
        ClassBounds::new(root.capacity_bytes(), plan(64 * 1024), plan(64 * 1024)).unwrap(),
    )
    .unwrap();
    for class in [
        IngressClass::Data,
        IngressClass::Transformation,
        IngressClass::Control,
    ] {
        drop(policy.admit(class, plan(1)).unwrap());
    }
    (root, policy)
}

#[test]
fn initial_refusal_allocates_no_future_and_never_invokes_the_source() {
    let (root, policy) = fixture();
    let available = policy.budget(IngressClass::Data).available_bytes();
    let pressure = policy
        .admit(IngressClass::Data, plan(available))
        .unwrap()
        .initialize(|| vec![0u8; available - 1024]);
    let initialized = AtomicBool::new(false);
    let mut probe = AllocationProbe::new();
    let failed: [_; 128] = std::array::from_fn(|_| {
        probe.track(|| {
            AdmittedOperation::new(&policy, IngressClass::Data, plan(8192), |_| {
                initialized.store(true, Ordering::Relaxed);
                async { Ok(vec![0u8; 8192]) }
            })
            .unwrap_err()
        })
    });
    assert!(failed.iter().all(|error| matches!(
        error,
        AdmittedError::Refused(AdmissionRefusal::AtCapacity { .. })
    )));
    assert!(!initialized.load(Ordering::Relaxed));
    assert_eq!(probe.allocated(), 0);
    drop((failed, pressure));
    root.assert_idle();
}

#[test]
fn isolated_source_future_and_result_keep_physical_storage_charged() {
    const TEST: &str = "ingress_budget::operation::tests::isolated_source_future_and_result_keep_physical_storage_charged";
    if std::env::var_os("CMA_SOURCE_OPERATION_CHILD").is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", TEST, "--nocapture", "--test-threads=1"])
            .env("CMA_SOURCE_OPERATION_CHILD", "1")
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
            AdmittedOperation::new(&policy, IngressClass::Data, plan(65_536), |_| async {
                Ok(vec![7u8; 65_536])
            })
            .unwrap()
        });
        let result = probe.track(|| (&mut operation).now_or_never().unwrap().unwrap());
        let charged = operation.requested_bytes();
        assert!(result.belongs_to(&policy));
        assert!(
            probe.peak() <= charged,
            "source peak={} admitted={charged}",
            probe.peak()
        );
        eprintln!(
            "source peak={} retained={} admitted={charged}",
            probe.peak(),
            probe.live()
        );
        if result_first {
            probe.track(|| drop(result));
            assert!(probe.live() > 0, "completed operation still owns its box");
            assert_eq!(root.reserved_work_bytes(), charged);
            probe.track(|| drop(operation));
        } else {
            let retained = probe.live();
            probe.track(|| drop(operation));
            assert!(probe.live() < retained, "completed future box was freed");
            assert!(probe.live() >= 65_536, "returned payload remains owned");
            assert_eq!(root.reserved_work_bytes(), charged);
            probe.track(|| drop(result));
        }
        assert_eq!(probe.live(), 0);
        root.assert_idle();
    }

    let (root, policy) = fixture();
    let mut probe = AllocationProbe::new();
    let mut unplanned = probe.track(|| {
        AdmittedOperation::new(&policy, IngressClass::Data, plan(0), |_| async {
            Ok(vec![7u8; 65_536])
        })
        .unwrap()
    });
    let result = probe.track(|| (&mut unplanned).now_or_never().unwrap().unwrap());
    assert!(
        probe.peak() > unplanned.requested_bytes(),
        "missing source work plan must fail the real allocation bound"
    );
    eprintln!(
        "unplanned source peak={} admitted={}",
        probe.peak(),
        unplanned.requested_bytes()
    );
    probe.track(|| drop((unplanned, result)));
    assert_eq!(probe.live(), 0);
    root.assert_idle();
}

#[test]
fn cancellation_frees_source_data_and_box_before_returning_the_allowance() {
    struct Payload {
        bytes: Vec<u8>,
        root: IngressBudget,
    }
    impl Drop for Payload {
        fn drop(&mut self) {
            assert!(
                self.root.available_bytes() <= self.root.capacity_bytes() - self.bytes.capacity()
            );
        }
    }
    let (root, policy) = fixture();
    let mut probe = AllocationProbe::new();
    let mut operation = probe.track(|| {
        AdmittedOperation::new(&policy, IngressClass::Data, plan(8192), |_| async {
            let payload = Payload {
                bytes: vec![0u8; 8192],
                root: root.clone(),
            };
            std::future::pending::<()>().await;
            Ok(payload)
        })
        .unwrap()
    });
    assert!(probe.track(|| (&mut operation).now_or_never()).is_none());
    assert!(probe.peak() <= operation.requested_bytes());
    probe.track(|| drop(operation));
    assert_eq!(probe.live(), 0);
    root.assert_idle();
}

#[test]
fn source_errors_keep_owned_diagnostics_after_the_future_is_gone() {
    let (root, policy) = fixture();
    let error = AdmittedOperation::new(&policy, IngressClass::Data, plan(0), |_| async {
        Err::<(), _>(super::super::diagnostic::make_preserving_category(
            crate::AdapterError::Unavailable,
            format_args!("source unavailable"),
        ))
    })
    .unwrap()
    .now_or_never()
    .unwrap()
    .unwrap_err();
    assert!(
        matches!(error.as_adapter(), Some(crate::AdapterError::Unavailable(detail)) if detail == "source unavailable")
    );
    let copy = error.clone();
    drop(error);
    assert_eq!(root.reserved_work_bytes(), copy.requested_bytes());
    drop(copy);
    root.assert_idle();
}

#[test]
fn compiler_layout_covers_a_large_inline_future_before_it_is_polled() {
    let (root, policy) = fixture();
    let mut probe = AllocationProbe::new();
    let mut operation = probe.track(|| {
        AdmittedOperation::new(&policy, IngressClass::Data, plan(0), |_| async {
            let bytes = [7u8; 8192];
            std::hint::black_box(&bytes);
            std::future::pending::<()>().await;
            Ok(bytes)
        })
        .unwrap()
    });
    assert!(probe.live() >= 8192);
    assert!(probe.peak() <= operation.requested_bytes());
    assert!(probe.track(|| (&mut operation).now_or_never()).is_none());
    assert!(probe.peak() <= operation.requested_bytes());
    probe.track(|| drop(operation));
    assert_eq!(probe.live(), 0);
    root.assert_idle();
}

fn custody_fixture() -> (IngressBudget, IngressOwnershipPolicy) {
    let root = IngressBudget::new(1024 * 1024);
    let bounds = ClassBounds::new(root.capacity_bytes(), plan(4096), plan(4096))
        .unwrap()
        .with_attachment_limit(2)
        .unwrap();
    let policy = IngressOwnershipPolicy::new(root.clone(), bounds).unwrap();
    (root, policy)
}

#[test]
fn operation_custody_domain_is_checked_before_polling_without_allocation() {
    use std::sync::atomic::AtomicUsize;
    let (root, process) = custody_fixture();
    let (foreign_root, foreign) = custody_fixture();
    let first = process.admit_attachment().unwrap();
    let second = process.admit_attachment().unwrap();
    let policies = [
        process.clone(),
        first.policy().clone(),
        second.policy().clone(),
        foreign,
    ];
    let calls = AtomicUsize::new(0);
    let operations = policies.each_ref().map(|policy| {
        let calls = &calls;
        AdmittedOperation::from_owned(
            policy,
            IngressClass::Data,
            plan(0),
            move |scope| async move {
                calls.fetch_add(1, Ordering::Relaxed);
                Ok(scope.initialize_owned(|| None::<usize>))
            },
        )
        .unwrap()
    });
    let available = root.available_bytes();
    let mut probe = AllocationProbe::new();
    probe.track(|| {
        for (source, operation) in operations.iter().enumerate() {
            for (target, policy) in policies.iter().enumerate() {
                assert_eq!(operation.shares_custody_domain(policy), source == target);
                assert_eq!(
                    operation.belongs_to(policy),
                    source == target || (target == 0 && matches!(source, 1 | 2))
                );
            }
        }
    });
    assert_eq!(probe.allocated(), 0);
    assert_eq!(calls.load(Ordering::Relaxed), 0);
    assert_eq!(root.available_bytes(), available);
    for (source, operation) in operations.into_iter().enumerate() {
        let result = operation.now_or_never().unwrap().unwrap();
        for (target, policy) in policies.iter().enumerate() {
            assert_eq!(result.shares_custody_domain(policy), source == target);
        }
    }
    assert_eq!(calls.load(Ordering::Relaxed), 4);
    drop((policies, first, second));
    root.assert_idle();
    foreign_root.assert_idle();
}

#[test]
fn operation_custody_does_not_certify_a_returned_envelope_or_dynamic_error() {
    let (root, process) = custody_fixture();
    let attachment = process.admit_attachment().unwrap();
    let attached_policy = attachment.policy().clone();
    let scope = IngressWorkScope::new(
        attached_policy
            .admit(IngressClass::Data, plan(1024))
            .unwrap(),
    );
    let result = scope.initialize_owned(|| None::<usize>);
    let operation =
        AdmittedOperation::from_owned(&process, IngressClass::Data, plan(0), |_| async move {
            Ok(result)
        })
        .unwrap();
    assert!(operation.shares_custody_domain(&process));
    assert!(!operation.shares_custody_domain(&attached_policy));
    let result = operation.now_or_never().unwrap().unwrap();
    assert!(result.belongs_to(&process));
    assert!(!result.shares_custody_domain(&process));
    assert!(result.shares_custody_domain(&attached_policy));
    drop((result, scope));

    let diagnostic = attached_policy
        .admit(IngressClass::Data, plan(1024))
        .unwrap()
        .initialize(|| crate::AdapterError::Unavailable("synthetic source failure".into()));
    let operation = AdmittedOperation::<()>::from_owned(
        &process,
        IngressClass::Data,
        plan(0),
        |_| async move { Err(AdmittedError::Adapter(diagnostic)) },
    )
    .unwrap();
    assert!(operation.shares_custody_domain(&process));
    let error = operation.now_or_never().unwrap().unwrap_err();
    assert!(error.belongs_to(&process));
    let AdmittedError::Adapter(diagnostic) = error else {
        panic!("expected owned diagnostic")
    };
    assert!(!diagnostic.shares_custody_domain(&process));
    assert!(diagnostic.shares_custody_domain(&attached_policy));
    let last = diagnostic.clone();
    drop((diagnostic, attachment, attached_policy));
    let second = process.admit_attachment().unwrap();
    assert!(process.admit_attachment().is_err());
    drop(last);
    drop(process.admit_attachment().unwrap());
    drop(second);
    root.assert_idle();
}
