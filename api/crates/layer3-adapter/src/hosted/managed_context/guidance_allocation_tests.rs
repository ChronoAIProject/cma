use super::fixture::{binding, policy};
use super::*;
use crate::allocation_probe::{deallocation, AllocationProbe};
use futures_util::FutureExt;

#[test]
fn guidance_transformation_admits_maximum_overlap_and_refuses_before_decoding() {
    if deallocation::isolated("hosted::managed_context::guidance_admission::allocation_tests::guidance_transformation_admits_maximum_overlap_and_refuses_before_decoding") { return; }
    let (root, policy) = policy();
    let original = format!("old\n\n{}", "x".repeat(MAX_INSTRUCTIONS - 5));
    let (binding, _) = binding(&policy, 0, &original, None);
    let status = binding
        .ready_status_admitted(&policy)
        .unwrap()
        .now_or_never()
        .unwrap()
        .unwrap();
    let file = crate::AgentFileReadReceipt {
        path: binding.0.path.clone(),
        size_bytes: original.len() as u64,
        sha256: format!("{:x}", Sha256::digest(original.as_bytes())),
        content_base64: base64::engine::general_purpose::STANDARD.encode(original.as_bytes()),
    };
    let previous = vec!["old".into()];
    let baseline = root.available_bytes();
    let mut probe = AllocationProbe::new();
    let prepared = probe.track(|| {
        binding
            .prepare_guidance(status.as_ref().unwrap(), &file, "new", &previous, &policy)
            .now_or_never()
            .unwrap()
            .unwrap()
    });
    let charged = baseline - root.available_bytes();
    assert!(probe.peak() >= 2 * MAX_INSTRUCTIONS);
    assert!(
        probe.peak() <= charged,
        "peak={} admitted={charged}",
        probe.peak()
    );
    eprintln!(
        "guidance_transformation peak={} retained={} admitted={charged}",
        probe.peak(),
        probe.live()
    );
    let retained = probe.track(|| retain_guidance(prepared, &policy).unwrap());
    assert_eq!(
        policy
            .budget(IngressClass::Transformation)
            .available_bytes(),
        policy.bounds().transformation_bytes()
    );
    let charged = baseline - root.available_bytes();
    assert!(probe.live() <= charged);
    let value = retained.as_ref().unwrap();
    assert_eq!(value.bytes, MAX_INSTRUCTIONS as u64);
    assert_eq!(value.request.expected_sha256.as_ref(), Some(&file.sha256));
    assert_eq!(
        value.request.content().unwrap(),
        original.replacen("old", "new", 1).as_bytes()
    );
    drop(retained);
    assert_eq!(probe.live(), 0);
    assert_eq!(root.available_bytes(), baseline);
    drop(probe);
    let pressure = policy
        .admit(
            IngressClass::Transformation,
            AllocationPlan::checked(
                "guidance pressure",
                &[policy
                    .budget(IngressClass::Transformation)
                    .available_bytes()],
            )
            .unwrap(),
        )
        .unwrap();
    let mut probe = AllocationProbe::new();
    let error = probe.track(|| {
        binding
            .prepare_guidance(status.as_ref().unwrap(), &file, "new", &previous, &policy)
            .now_or_never()
            .unwrap()
            .err()
            .unwrap()
    });
    assert!(error.refusal().is_some());
    assert_eq!(probe.allocated(), 0);
    drop((error, probe));
    let mut probe = AllocationProbe::new();
    let legacy = probe.track(|| file.content().unwrap());
    assert!(probe.peak() >= MAX_INSTRUCTIONS);
    assert_eq!(
        policy
            .budget(IngressClass::Transformation)
            .available_bytes(),
        0
    );
    drop((legacy, pressure, status, binding));
    assert_eq!(probe.live(), 0);
    root.assert_idle();
}

#[test]
fn guidance_retention_refusal_frees_replacement_before_any_write() {
    let (root, policy) = policy();
    let (binding, source) = binding(&policy, 0, "owner", None);
    let status = binding
        .ready_status_admitted(&policy)
        .unwrap()
        .now_or_never()
        .unwrap()
        .unwrap();
    let file = crate::AgentFileReadReceipt {
        path: binding.0.path.clone(),
        size_bytes: 5,
        sha256: format!("{:x}", Sha256::digest(b"owner")),
        content_base64: base64::engine::general_purpose::STANDARD.encode("owner"),
    };
    let prepared = binding
        .prepare_guidance(status.as_ref().unwrap(), &file, "new", &[], &policy)
        .now_or_never()
        .unwrap()
        .unwrap();
    let pressure = policy
        .admit(
            IngressClass::Data,
            AllocationPlan::checked(
                "guidance retained pressure",
                &[policy.budget(IngressClass::Data).available_bytes()],
            )
            .unwrap(),
        )
        .unwrap();
    let mut probe = AllocationProbe::new();
    let error = probe.track(|| retain_guidance(prepared, &policy).err().unwrap());
    assert!(error.refusal().is_some());
    assert_eq!(probe.allocated(), 0);
    assert_eq!(source.writes.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert_eq!(*source.text.lock().unwrap(), "owner");
    assert_eq!(
        policy
            .budget(IngressClass::Transformation)
            .available_bytes(),
        policy.bounds().transformation_bytes()
    );
    drop((pressure, status, binding, source));
    root.assert_idle();
}
