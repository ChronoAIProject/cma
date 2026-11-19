use super::*;
use crate::allocation_probe::AllocationProbe;
use crate::ingress_budget::{
    AllocationPlan, ClassBounds, IngressBudget, IngressClass, IngressOwnershipPolicy,
};

fn plan(bytes: usize) -> AllocationPlan {
    AllocationPlan::new("failure fixture", &[bytes]).unwrap()
}

fn policy() -> (IngressBudget, IngressOwnershipPolicy) {
    let root = IngressBudget::new(128 * 1024);
    let policy = IngressOwnershipPolicy::new(
        root.clone(),
        ClassBounds::new(root.capacity_bytes(), plan(16 * 1024), plan(32 * 1024)).unwrap(),
    )
    .unwrap();
    (root, policy)
}

#[test]
fn isolated_admission_refusals_allocate_nothing_when_retained_or_cloned() {
    const TEST: &str = "ingress_budget::failure::tests::isolated_admission_refusals_allocate_nothing_when_retained_or_cloned";
    if std::env::var_os("CMA_ADMISSION_FAILURE_CHILD").is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", TEST, "--nocapture", "--test-threads=1"])
            .env("CMA_ADMISSION_FAILURE_CHILD", "1")
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
    let (root, policy) = policy();
    for class in [
        IngressClass::Data,
        IngressClass::Transformation,
        IngressClass::Control,
    ] {
        let available = policy.budget(class).available_bytes();
        let held = policy
            .reserve(class, plan(available))
            .unwrap()
            .initialize(|| vec![0u8; available - 512]);
        let baseline = root.available_bytes();
        let mut probe = AllocationProbe::new();
        let legacy: [AdapterError; 128] =
            probe.track(|| std::array::from_fn(|_| policy.reserve(class, plan(1)).unwrap_err()));
        assert!(probe.live() >= 128 * "ingress allocation capacity exhausted".len());
        eprintln!(
            "legacy_refusals class={class:?} requested_peak={} retained={}",
            probe.peak(),
            probe.live()
        );
        drop(legacy);
        assert_eq!(probe.live(), 0);
        drop(probe);
        let mut probe = AllocationProbe::new();
        let errors: [AdmittedError; 128] = probe
            .track(|| std::array::from_fn(|_| policy.admit(class, plan(1)).unwrap_err().into()));
        let clones = probe.track(|| errors.clone());
        assert_eq!(
            probe.allocated(),
            0,
            "converting a legacy allocated error is too late"
        );
        eprintln!(
            "admitted_refusals class={class:?} requested_peak={} retained={} churn={}",
            probe.peak(),
            probe.live(),
            probe.allocated()
        );
        assert_eq!(probe.live(), 0);
        assert!(clones
            .iter()
            .all(|error| error.as_adapter().is_none() && error.requested_bytes() == 0));
        assert_eq!(root.available_bytes(), baseline);
        drop((clones, errors, probe, held));
        root.assert_idle();
    }
    let mut storage = policy.admit(IngressClass::Data, plan(1024)).unwrap();
    let mut probe = AllocationProbe::new();
    let invalid_plan = probe.track(|| AllocationPlan::checked("overflow", &[usize::MAX, 1]));
    let invalid_split = probe.track(|| storage.split_admitted(plan(1025)));
    assert!(matches!(
        invalid_plan,
        Err(AdmissionRefusal::InvalidPlan { .. })
    ));
    assert!(matches!(
        invalid_split,
        Err(AdmissionRefusal::InvalidPlan { .. })
    ));
    assert_eq!(probe.allocated(), 0);
    assert_eq!(storage.requested_bytes(), 1024);
    drop((probe, storage));
    root.assert_idle();
}

#[test]
fn runtime_error_is_borrowed_and_charged_until_the_final_result_owner_drops() {
    let (root, policy) = policy();
    let requested = 65_536 + crate::ingress_budget::sizing::shared_owner_bytes::<AdapterError>();
    let error: AdmittedError = policy
        .admit(IngressClass::Data, plan(requested))
        .unwrap()
        .initialize(|| AdapterError::OperationIdentityConflict("x".repeat(65_536)))
        .into();
    assert_eq!(error.requested_bytes(), requested);
    let remaining = error.clone();
    assert!(std::ptr::eq(
        error.as_adapter().unwrap(),
        remaining.as_adapter().unwrap()
    ));
    assert!(error.refusal().is_none());
    assert!(std::error::Error::source(&error).is_some());
    drop(error);
    assert_eq!(root.reserved_work_bytes(), requested);
    assert!(
        matches!(remaining.as_adapter(), Some(AdapterError::OperationIdentityConflict(message)) if message.len() == 65_536)
    );
    drop(remaining);
    root.assert_idle();
}

#[test]
fn root_refusal_rolls_back_the_class_and_legacy_errors_keep_their_classification() {
    let (root, policy) = policy();
    let held = root.try_reserve(root.available_bytes()).unwrap();
    assert!(matches!(
        policy.admit(IngressClass::Control, plan(1)),
        Err(AdmissionRefusal::AtCapacity { .. })
    ));
    drop(held);
    let control = policy
        .admit(IngressClass::Control, plan(policy.bounds().control_bytes()))
        .unwrap();
    let legacy = policy.reserve(IngressClass::Control, plan(1)).unwrap_err();
    assert_eq!(
        legacy,
        AdapterError::AtCapacity("ingress allocation capacity exhausted".into())
    );
    assert_eq!(
        AllocationPlan::new("overflow", &[usize::MAX, 1]).unwrap_err(),
        AdapterError::Rejected("ingress allocation plan exceeds its supported bounds".into())
    );
    drop(control);
    root.assert_idle();
}
