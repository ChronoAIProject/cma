use super::*;
use crate::sandbox_metrics::{Binding, BindingKind, ProofStatus, Range};
use layer3_adapter::sandbox_metrics::{PhysicalSeries, Sample, SourceMetric};
use std::collections::BTreeMap;

mod support;

fn expected() -> ExpectedMinute {
    ExpectedMinute {
        sandbox_id: "sandbox-fixture".into(),
        owner: "owner-fixture".into(),
        bucket_start: 0,
        compute: vec![Range { from: 0, to: 60 }],
        home: vec![Range { from: 0, to: 60 }],
    }
}

fn binding(kind: BindingKind) -> Binding {
    Binding {
        id: format!("binding-{kind:?}"),
        schema_version: 1,
        revision: 1,
        deployment: "fixture".into(),
        cluster: "fixture".into(),
        namespace: "fixture".into(),
        native_sandbox_uid: "native-fixture".into(),
        claim_uid: "claim-fixture".into(),
        sandbox_id: "sandbox-fixture".into(),
        owner_subject: "owner-fixture".into(),
        pod_name: Some("pod-fixture".into()),
        pod_uid: Some("pod-uid-fixture".into()),
        runtime_container_id: Some("container-fixture".into()),
        restart_count: Some(0),
        runtime_generation: Some(1),
        pvc_name: "home-fixture".into(),
        pvc_uid: "home-uid-fixture".into(),
        kind,
        cpu_limit_cores: Some(2.0),
        memory_limit_bytes: Some(1024.0),
        valid_from: 0,
        valid_until: Some(180),
        observed_at: 180,
        proof_version: 1,
        proof_status: ProofStatus::Verified,
    }
}

fn series(metric: SourceMetric, values: &[(f64, f64)]) -> PhysicalSeries {
    PhysicalSeries {
        metric,
        name: if metric.is_pvc() {
            "home-fixture"
        } else {
            "pod-fixture"
        }
        .into(),
        labels: BTreeMap::from([
            ("cluster".into(), "fixture".into()),
            ("namespace".into(), "fixture".into()),
        ]),
        samples: values
            .iter()
            .map(|&(at, value)| Sample {
                at,
                value: Some(value),
            })
            .collect(),
    }
}

fn freeze(values: Vec<PhysicalSeries>) -> FrozenMeasuredMinute {
    freeze_measured(
        expected(),
        vec![binding(BindingKind::Compute), binding(BindingKind::Home)],
        values,
        true,
    )
    .unwrap()
}

#[test]
fn observed_zero_and_missing_samples_remain_different() {
    let value = freeze(vec![series(
        SourceMetric::CpuCounter,
        &[(0.0, 7.0), (60.0, 7.0)],
    )]);
    assert_eq!(value.fact().metrics[0].quantity.as_deref(), Some("0"));
    assert_eq!(value.fact().metrics[0].quality, MeasuredQuality::Observed);
    assert_eq!(value.fact().metrics[1].quantity, None);
    assert_eq!(value.fact().metrics[2].quantity, None);
    assert_eq!(value.fact().metrics[1].expected_binding_ms, "60000");
    assert_eq!(value.fact().metrics[1].quality, MeasuredQuality::Missing);
}

#[test]
fn unbound_lifecycle_and_paused_home_are_retained_as_missing() {
    let mut expectation = expected();
    expectation.compute.clear();
    let value = freeze_measured(expectation, vec![], vec![], true).unwrap();
    assert_eq!(value.fact().compute_uptime_ms, 0);
    assert_eq!(value.fact().retained_home_ms, 60000);
    assert_eq!(value.fact().metrics[0].quantity.as_deref(), Some("0"));
    assert_eq!(value.fact().metrics[2].quantity, None);
    assert_eq!(
        value.fact().metrics[2].missing_reason.as_deref(),
        Some("binding_missing")
    );
}

#[test]
fn counter_reset_and_gap_use_observed_subtotals_without_extrapolation() {
    let reset = freeze(vec![series(
        SourceMetric::CpuCounter,
        &[(0.0, 0.0), (30.0, 30.0), (60.0, 2.0)],
    )]);
    assert_eq!(
        reset.fact().metrics[0].quantity.as_deref(),
        Some("30000000000")
    );
    assert_eq!(reset.fact().metrics[0].covered_binding_ms, "30000");
    assert_eq!(reset.fact().metrics[0].quality, MeasuredQuality::Partial);
    let mut expectation = expected();
    expectation.bucket_start = 60;
    expectation.compute = vec![Range { from: 60, to: 120 }];
    expectation.home.clear();
    for (end, covered) in [(90.0, "30000"), (91.0, "0")] {
        let value = freeze_measured(
            expectation.clone(),
            vec![binding(BindingKind::Compute)],
            vec![series(SourceMetric::CpuCounter, &[(0.0, 0.0), (end, end)])],
            true,
        )
        .unwrap();
        assert_eq!(value.fact().metrics[0].covered_binding_ms, covered);
    }
}

#[test]
fn gauge_hold_stops_after_ninety_seconds() {
    let mut expectation = expected();
    expectation.bucket_start = 60;
    expectation.compute = vec![Range { from: 60, to: 120 }];
    expectation.home.clear();
    let value = freeze_measured(
        expectation,
        vec![binding(BindingKind::Compute)],
        vec![series(SourceMetric::WorkingSet, &[(0.0, 100.0)])],
        true,
    )
    .unwrap();
    assert_eq!(value.fact().metrics[1].quantity.as_deref(), Some("3000000"));
    assert_eq!(value.fact().metrics[1].covered_binding_ms, "30000");
    assert_eq!(value.fact().metrics[1].quality, MeasuredQuality::Partial);
}

#[test]
fn identical_home_observations_deduplicate_but_contradictions_are_refused() {
    let home = binding(BindingKind::Home);
    let value = freeze_measured(
        expected(),
        vec![home.clone(), home.clone()],
        vec![series(SourceMetric::PvcUsed, &[(0.0, 100.0)])],
        true,
    )
    .unwrap();
    assert_eq!(value.fact().metrics[2].quantity.as_deref(), Some("6000000"));
    let mut different = home.clone();
    different.pvc_uid = "different-home".into();
    assert_eq!(
        freeze_measured(expected(), vec![home, different], vec![], true),
        Err(MeasuredError::Attribution)
    );
}

#[test]
fn historical_cross_owner_physical_overlap_is_not_filtered_away() {
    let own = binding(BindingKind::Home);
    let mut other = own.clone();
    other.id = "foreign-binding".into();
    other.sandbox_id = "foreign-sandbox".into();
    other.owner_subject = "foreign-owner".into();
    assert_eq!(
        freeze_measured(expected(), vec![own, other], vec![], true),
        Err(MeasuredError::Attribution)
    );
}

#[test]
fn concurrent_physical_bindings_and_unbound_time_both_count_in_expectations() {
    let mut first = binding(BindingKind::Compute);
    first.valid_until = Some(30);
    let mut second = first.clone();
    second.id = "second-compute".into();
    second.pod_uid = Some("second-pod-uid".into());
    second.pod_name = Some("second-pod".into());
    let memory = series(SourceMetric::WorkingSet, &[(0.0, 100.0)]);
    let mut memory2 = memory.clone();
    memory2.name = "second-pod".into();
    let value =
        freeze_measured(expected(), vec![first, second], vec![memory, memory2], true).unwrap();
    assert_eq!(value.fact().metrics[1].expected_binding_ms, "90000");
    assert_eq!(value.fact().metrics[1].covered_binding_ms, "60000");
    assert_eq!(value.fact().metrics[1].quantity.as_deref(), Some("6000000"));
}

#[test]
fn a_new_incarnation_never_borrows_the_previous_counter_baseline() {
    let mut before = binding(BindingKind::Compute);
    before.valid_until = Some(30);
    let mut after = before.clone();
    after.id = "new-incarnation".into();
    after.valid_from = 30;
    after.valid_until = Some(180);
    after.restart_count = Some(1);
    after.runtime_container_id = Some("new-container".into());
    let value = freeze_measured(
        expected(),
        vec![before, after],
        vec![series(
            SourceMetric::CpuCounter,
            &[(0.0, 100.0), (60.0, 160.0)],
        )],
        true,
    )
    .unwrap();
    assert_eq!(value.fact().metrics[0].quantity, None);
    assert_eq!(value.fact().metrics[0].quality, MeasuredQuality::Missing);
}

#[test]
fn incomplete_source_response_cannot_be_reported_as_observed() {
    let value = freeze_measured(
        expected(),
        vec![binding(BindingKind::Home)],
        vec![series(SourceMetric::PvcUsed, &[(0.0, 100.0)])],
        false,
    )
    .unwrap();
    assert_eq!(value.fact().metrics[2].quantity, None);
    assert_eq!(
        value.fact().metrics[2].missing_reason.as_deref(),
        Some("source_unavailable")
    );
    value.verify_replay().unwrap();
}

#[test]
fn coverage_rounding_does_not_hide_a_fractional_gap() {
    let value = freeze(vec![series(SourceMetric::WorkingSet, &[(0.0001, 1.0)])]);
    assert_eq!(value.fact().metrics[1].covered_binding_ms, "60000");
    assert_eq!(value.fact().metrics[1].expected_binding_ms, "60000");
    assert_eq!(value.fact().metrics[1].quality, MeasuredQuality::Estimated);
    assert_eq!(
        value.fact().metrics[1].missing_reason.as_deref(),
        Some("coverage_quantized")
    );
}

#[test]
fn frozen_bits_and_encoded_result_survive_serialization_and_replay() {
    let value = freeze(vec![series(
        SourceMetric::WorkingSet,
        &[(0.0, 9_007_199_254_740_992.0)],
    )]);
    let bytes = serde_json::to_vec(&value).unwrap();
    let restored: FrozenMeasuredMinute = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(serde_json::to_vec(&restored).unwrap(), bytes);
    restored.verify_replay().unwrap();
    assert_eq!(
        restored.fact().metrics[1].quantity.as_deref(),
        Some("540431955284459520000")
    );
    let mut altered = serde_json::to_value(value).unwrap();
    altered["series"][0]["samples"][0][1]["value"] = serde_json::json!(1.0);
    assert!(serde_json::from_value::<FrozenMeasuredMinute>(altered).is_err());
}

#[test]
fn overflow_and_nonfinite_samples_cannot_be_frozen() {
    for value in [f64::INFINITY, f64::NAN, -1.0, 1e307] {
        assert!(freeze_measured(
            expected(),
            vec![binding(BindingKind::Compute)],
            vec![series(SourceMetric::WorkingSet, &[(0.0, value)])],
            true
        )
        .is_err());
    }
}

#[test]
fn observed_residual_compute_remains_measured_when_lifecycle_compute_is_off() {
    let mut expectation = expected();
    expectation.compute.clear();
    let value = freeze_measured(
        expectation,
        vec![binding(BindingKind::Compute)],
        vec![series(
            SourceMetric::CpuCounter,
            &[(0.0, 10.0), (60.0, 70.0)],
        )],
        true,
    )
    .unwrap();
    assert_eq!(value.fact().compute_uptime_ms, 0);
    assert_eq!(
        value.fact().metrics[0].quantity.as_deref(),
        Some("60000000000")
    );
    assert_eq!(value.fact().metrics[0].expected_binding_ms, "60000");
    value.verify_replay().unwrap();
}

#[test]
fn explicit_repair_recomputes_original_inputs_without_mutating_original_results() {
    let value = freeze(vec![series(
        SourceMetric::CpuCounter,
        &[(0.0, 1.0), (60.0, 61.0)],
    )]);
    let mut old = serde_json::to_value(&value).unwrap();
    old["fact"]["metrics"][0]["quantity"] = "1".into();
    let retained: FrozenMeasuredMinute = serde_json::from_value(old).unwrap();
    let bytes = serde_json::to_vec(&retained).unwrap();
    assert!(retained.verify_replay().is_err());
    assert_eq!(retained.recompute_retained().unwrap(), value);
    assert_eq!(serde_json::to_vec(&retained).unwrap(), bytes);
}
