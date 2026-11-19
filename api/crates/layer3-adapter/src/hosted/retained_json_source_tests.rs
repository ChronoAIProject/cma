use super::*;
use crate::allocation_probe::{deallocation, AllocationProbe};
use crate::ingress_budget::{
    AdmissionRefusal, AllocationPlan, ClassBounds, IngressClass, IngressOwnershipPolicy,
};

#[test]
fn isolated_source_decode_keeps_the_tree_after_raw_output_and_scratch_drop() {
    if deallocation::isolated("hosted::retained_json::source_tests::isolated_source_decode_keeps_the_tree_after_raw_output_and_scratch_drop") { return; }
    let root = IngressBudget::new(128 * 1024);
    let floor = AllocationPlan::checked("source decoder floor", &[16 * 1024]).unwrap();
    let policy = IngressOwnershipPolicy::new(
        root.clone(),
        ClassBounds::new(root.capacity_bytes(), floor, floor).unwrap(),
    )
    .unwrap();
    for class in [IngressClass::Data, IngressClass::Transformation] {
        drop(
            policy
                .admit(
                    class,
                    AllocationPlan::checked("policy backing", &[1]).unwrap(),
                )
                .unwrap(),
        );
    }
    let raw = serde_json::to_vec(&serde_json::json!({"text": "x".repeat(4096)})).unwrap();
    let mut probe = AllocationProbe::new();
    let tree = probe.track(|| JsonFrame::decode_admitted(&raw, &policy).unwrap());
    assert_eq!(tree["text"].as_str().unwrap().len(), 4096);
    let retained = root.capacity_bytes() - root.available_bytes();
    assert!(probe.live() >= 4096);
    assert!(probe.live() <= retained);
    assert!(probe.peak() <= retained + 3 * raw.len() + FIXED_ALLOWANCE);
    let clone = probe.track(|| tree.clone());
    drop((tree, raw));
    assert!(probe.live() >= 4096);
    assert_eq!(
        policy
            .budget(IngressClass::Transformation)
            .available_bytes(),
        16 * 1024
    );
    drop(clone);
    assert_eq!(probe.live(), 0);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
    drop(probe);

    let raw = serde_json::to_vec(&serde_json::json!({"text": "x".repeat(4096)})).unwrap();
    let pressure = policy
        .admit(IngressClass::Transformation, floor)
        .unwrap()
        .initialize(|| vec![0_u8; 15 * 1024]);
    let mut probe = AllocationProbe::new();
    let legacy: Value = probe.track(|| serde_json::from_slice(&raw).unwrap());
    assert!(probe.live() >= 4096);
    drop(legacy);
    assert_eq!(probe.live(), 0);
    drop(probe);
    let mut probe = AllocationProbe::new();
    let error = probe.track(|| JsonFrame::decode_admitted(&raw, &policy).unwrap_err());
    assert!(matches!(
        error.refusal(),
        Some(AdmissionRefusal::AtCapacity { .. })
    ));
    assert_eq!(probe.allocated(), 0);
    drop((probe, pressure));
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}
