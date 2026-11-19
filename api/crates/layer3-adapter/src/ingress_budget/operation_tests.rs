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
    assert_eq!(root.available_bytes(), root.capacity_bytes());
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
            assert_eq!(root.capacity_bytes() - root.available_bytes(), charged);
            probe.track(|| drop(operation));
        } else {
            let retained = probe.live();
            probe.track(|| drop(operation));
            assert!(probe.live() < retained, "completed future box was freed");
            assert!(probe.live() >= 65_536, "returned payload remains owned");
            assert_eq!(root.capacity_bytes() - root.available_bytes(), charged);
            probe.track(|| drop(result));
        }
        assert_eq!(probe.live(), 0);
        assert_eq!(root.available_bytes(), root.capacity_bytes());
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
    assert_eq!(root.available_bytes(), root.capacity_bytes());
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
    assert_eq!(root.available_bytes(), root.capacity_bytes());
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
    assert_eq!(
        root.capacity_bytes() - root.available_bytes(),
        copy.requested_bytes()
    );
    drop(copy);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
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
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}
