use super::*;
use crate::sandbox_metrics::{
    measured_evidence::{ObservationV1, PhysicalIdentity, RetainedObservation},
    tests::binding,
    Binding, BindingKind, SOURCE,
};
use crate::sandbox_usage::{Allocation, UsageEventKind};
use crate::{SandboxId, SubjectId};
use num_bigint::BigUint;
use num_rational::Ratio;

fn sample(
    binding: &Binding,
    metric: SourceMetric,
    timestamp_ms: u64,
    value: f64,
) -> RetainedObservation {
    let physical = PhysicalIdentity::for_binding(binding, metric, 1).unwrap();
    let physical_sample_id = physical.sample_id(binding, metric, timestamp_ms).unwrap();
    let observation = ObservationV1 {
        schema_version: 1,
        source: SOURCE.into(),
        metric,
        timestamp_ms,
        source_value_be: value.to_be_bytes(),
        physical_sample_id,
        mapping_revision: 1,
        binding: binding.clone(),
    };
    RetainedObservation::decode(
        &serde_json::to_vec(&observation).unwrap(),
        &observation.physical_sample_id,
        &digest(&observation).unwrap(),
    )
    .unwrap()
}

fn allocation(running: bool) -> Allocation {
    Allocation {
        running,
        cpu_millis: if running { 1000 } else { 0 },
        memory_mib: if running { 2 } else { 0 },
        disk_gib: 1,
    }
}

fn event(revision: u64, from: u64, to: u64, before: Allocation, next: Allocation) -> UsageEvent {
    UsageEvent {
        sandbox_id: SandboxId("sbx".into()),
        owner: SubjectId::new("owner-a"),
        revision,
        source_seq: revision,
        kind: UsageEventKind::Checkpoint,
        started_at: from,
        ended_at: to,
        quantity: before.usage(to - from).unwrap(),
        allocation: before,
        next_allocation: next,
        status: "active".into(),
    }
}

fn q(n: u64, d: u64) -> Option<ExactQuantity> {
    Some(Ratio::new(BigUint::from(n), BigUint::from(d)).into())
}

fn settle(
    samples: &[RetainedObservation],
    events: &[UsageEvent],
    from: u64,
    to: u64,
    cutoff: u64,
) -> Vec<CoverageInterval> {
    let custody: Vec<_> = samples.iter().map(CustodySample::Undisputed).collect();
    normalize_coverage("owner-a", "sbx", &custody, events, from, to, cutoff).unwrap()
}

#[test]
fn coverage_preserves_exact_clipping_pause_and_unmounted_pvc_pending() {
    let compute = binding(BindingKind::Compute);
    let home = binding(BindingKind::Home);
    let events = [
        event(1, 0, 60, allocation(true), allocation(false)),
        event(2, 60, 180, allocation(false), allocation(false)),
    ];
    let raw = [
        sample(&compute, SourceMetric::CpuCounter, 0, 0.0),
        sample(&compute, SourceMetric::CpuCounter, 60_000, 30.0),
        sample(&compute, SourceMetric::WorkingSet, 0, 1024.0),
        sample(&compute, SourceMetric::WorkingSet, 60_000, 2048.0),
        sample(&home, SourceMetric::PvcUsed, 0, 100.0),
        sample(&home, SourceMetric::PvcUsed, 60_000, 200.0),
        sample(&home, SourceMetric::PvcUsed, 120_000, 300.0),
    ];
    let rows = settle(&raw, &events, 30_000, 150_000, 150_000);
    let find = |metric, from| {
        rows.iter()
            .find(|row| row.metric == metric && row.from_ms == from)
            .unwrap()
    };
    let cpu = find(SourceMetric::CpuCounter, 30_000);
    assert_eq!(cpu.quantity, q(15, 1));
    assert_eq!(cpu.allocated_quantity, q(30, 1));
    assert_eq!(
        cpu.derivation,
        Some(QuantityDerivation::ProportionalCounterDelta)
    );
    assert_eq!(cpu.measurement_status, MeasurementStatus::Estimated);
    assert_eq!(
        find(SourceMetric::WorkingSet, 30_000).quantity,
        q(1024 * 30_000, 1)
    );
    let paused = find(SourceMetric::CpuCounter, 60_000);
    assert_eq!(paused.measurement_status, MeasurementStatus::NotRunning);
    assert_eq!(paused.expected_ms, 0);
    assert_eq!(paused.quantity, None);
    let disk = find(SourceMetric::PvcUsed, 60_000);
    assert_eq!(disk.measurement_status, MeasurementStatus::Measured);
    assert_eq!(disk.quantity, q(200 * 60_000, 1));
    let pending = find(SourceMetric::PvcUsed, 120_000);
    assert_eq!(pending.measurement_status, MeasurementStatus::Pending);
    assert_eq!(pending.expected_ms, 30_000);
    assert_eq!(pending.quantity, None);
}

#[test]
fn late_interior_gap_interpolation_is_exact_bounded_and_capped() {
    let compute = binding(BindingKind::Compute);
    let raw: Vec<_> = [
        (0, 0.0),
        (60_000, 120.0),
        (180_000, 240.0),
        (240_000, 360.0),
    ]
    .into_iter()
    .map(|(time, value)| sample(&compute, SourceMetric::CpuCounter, time, value))
    .collect();
    let events = [event(1, 0, 300, allocation(true), allocation(true))];
    let baseline = settle(&raw, &events, 0, 300_000, 300_000);
    let later = settle(&raw, &events, 0, 300_000, 180_000 + LATE_GRACE_MS);
    let gap = |rows: &[CoverageInterval]| {
        rows.iter()
            .find(|row| row.metric == SourceMetric::CpuCounter && row.from_ms == 60_000)
            .unwrap()
            .clone()
    };
    assert_eq!(
        gap(&baseline).measurement_status,
        MeasurementStatus::Pending
    );
    let estimated = gap(&later);
    assert_eq!(estimated.measurement_status, MeasurementStatus::Estimated);
    assert_eq!(estimated.quantity, q(120, 1));
    assert_eq!(
        estimated.derivation,
        Some(QuantityDerivation::NeighboringRates)
    );
    assert_eq!(estimated.source_references.len(), 4);
    assert_eq!(
        later
            .iter()
            .find(|row| row.metric == SourceMetric::CpuCounter && row.from_ms == 240_000)
            .unwrap()
            .measurement_status,
        MeasurementStatus::Pending
    );
}

#[test]
fn disputed_and_foreign_samples_cannot_bridge_a_covered_interval() {
    let compute = binding(BindingKind::Compute);
    let mut other = compute.clone();
    other.owner_subject = "owner-b".into();
    let a = sample(&compute, SourceMetric::WorkingSet, 0, 100.0);
    let b = sample(&other, SourceMetric::WorkingSet, 30_000, 150.0);
    let c = sample(&compute, SourceMetric::WorkingSet, 60_000, 200.0);
    let events = [event(1, 0, 60, allocation(true), allocation(true))];
    for mid in [CustodySample::Undisputed(&b), CustodySample::Disputed(&b)] {
        let custody = [
            CustodySample::Undisputed(&a),
            mid,
            CustodySample::Undisputed(&c),
        ];
        let rows =
            normalize_coverage("owner-a", "sbx", &custody, &events, 0, 60_000, 60_000).unwrap();
        assert!(rows
            .iter()
            .filter(|row| row.metric == SourceMetric::WorkingSet)
            .all(|row| row.measurement_status == MeasurementStatus::Unmapped
                && row.quantity.is_none()
                && row.source_references.is_empty()));
    }
}

#[test]
fn overlapping_physical_series_are_unmapped_instead_of_double_counted() {
    let first = binding(BindingKind::Home);
    let mut second = first.clone();
    second.pvc_uid = "other-volume".into();
    second.id = "other-binding".into();
    let raw = [
        sample(&first, SourceMetric::PvcUsed, 0, 100.0),
        sample(&first, SourceMetric::PvcUsed, 60_000, 100.0),
        sample(&second, SourceMetric::PvcUsed, 0, 200.0),
        sample(&second, SourceMetric::PvcUsed, 60_000, 200.0),
    ];
    let events = [event(1, 0, 60, allocation(false), allocation(false))];
    let rows = settle(&raw, &events, 0, 60_000, 60_000);
    let disk = rows
        .iter()
        .find(|row| row.metric == SourceMetric::PvcUsed)
        .unwrap();
    assert_eq!(disk.measurement_status, MeasurementStatus::Unmapped);
    assert_eq!(disk.expected_ms, 60_000);
    assert_eq!(disk.quantity, None);
}

#[test]
fn frozen_batches_replay_stably_and_bind_corrections_to_original_scope() {
    use crate::sandbox_metrics::measured_evidence::batch::{EvidenceScope, FrozenEvidence};
    let scope = EvidenceScope {
        owner_id: "owner-a".into(),
        sandbox_id: "sbx".into(),
        window_id: "window-fixture".into(),
        from_ms: 0,
        to_ms: 60_000,
        rate_card_id: "rate-fixture".into(),
        rate_card_version: 1,
    };
    let events = [event(1, 0, 60, allocation(true), allocation(true))];
    let before = FrozenEvidence::freeze(scope.clone(), 60_000, &[], &events, None).unwrap();
    let bytes = serde_json::to_vec(&before).unwrap();
    assert_eq!(FrozenEvidence::decode(&bytes).unwrap(), before);
    assert_eq!(
        FrozenEvidence::freeze(scope.clone(), 60_000, &[], &events, None).unwrap(),
        before
    );
    let home = binding(BindingKind::Home);
    let raw = [
        sample(&home, SourceMetric::PvcUsed, 0, 100.0),
        sample(&home, SourceMetric::PvcUsed, 60_000, 200.0),
    ];
    let samples = raw
        .iter()
        .map(CustodySample::Undisputed)
        .collect::<Vec<_>>();
    let corrected =
        FrozenEvidence::freeze(scope.clone(), 60_000, &samples, &events, Some(&before)).unwrap();
    assert_eq!(corrected.revision(), 2);
    corrected.validate_successor(&before).unwrap();
    assert_ne!(corrected.evidence_id(), before.evidence_id());
    assert_eq!(serde_json::to_vec(&before).unwrap(), bytes);
    assert!(
        FrozenEvidence::freeze(scope.clone(), 60_000, &samples, &events, Some(&corrected)).is_err()
    );
    let mut other = scope;
    other.rate_card_version = 2;
    assert!(FrozenEvidence::freeze(other, 60_000, &samples, &events, Some(&before)).is_err());
}

#[test]
fn resets_and_ambiguous_neighbors_cannot_fund_an_interior_estimate() {
    let compute = binding(BindingKind::Compute);
    let events = [event(1, 0, 300, allocation(true), allocation(true))];
    let gap = |rows: Vec<CoverageInterval>| {
        rows.into_iter()
            .find(|row| row.metric == SourceMetric::CpuCounter && row.from_ms == 60_000)
            .unwrap()
    };
    let reset = [(0, 0.0), (60_000, 60.0), (180_000, 5.0), (240_000, 65.0)]
        .map(|(time, value)| sample(&compute, SourceMetric::CpuCounter, time, value));
    let row = gap(settle(&reset, &events, 0, 300_000, 180_000 + LATE_GRACE_MS));
    assert_eq!(row.reason, Some(CoverageReason::CounterReset));
    assert_eq!(row.quantity, None);
    let mut another = compute.clone();
    another.id = "other-compute".into();
    another.pod_uid = Some("other-pod".into());
    another.runtime_container_id = Some("other-container".into());
    let mut raw = [(0, 0.0), (60_000, 60.0), (180_000, 180.0), (240_000, 240.0)]
        .into_iter()
        .map(|(time, value)| sample(&compute, SourceMetric::CpuCounter, time, value))
        .collect::<Vec<_>>();
    raw.extend([
        sample(&another, SourceMetric::CpuCounter, 0, 0.0),
        sample(&another, SourceMetric::CpuCounter, 60_000, 60.0),
    ]);
    let row = gap(settle(&raw, &events, 0, 300_000, 180_000 + LATE_GRACE_MS));
    assert_eq!(row.measurement_status, MeasurementStatus::Pending);
    assert_eq!(row.quantity, None);
}

#[test]
fn interrupted_disk_allocation_and_overlapping_series_invalidate_the_entire_gap() {
    let home = binding(BindingKind::Home);
    let disk = allocation(false);
    let mut no_disk = disk.clone();
    no_disk.disk_gib = 0;
    let mut raw = [(0, 10.0), (60_000, 10.0), (180_000, 30.0), (240_000, 30.0)]
        .into_iter()
        .map(|(time, value)| sample(&home, SourceMetric::PvcUsed, time, value))
        .collect::<Vec<_>>();
    let interrupted = [
        event(1, 0, 90, disk.clone(), no_disk.clone()),
        event(2, 90, 150, no_disk, disk.clone()),
        event(3, 150, 240, disk.clone(), disk.clone()),
    ];
    let check = |rows: Vec<CoverageInterval>, middle| {
        let gap = rows
            .iter()
            .filter(|row| {
                row.metric == SourceMetric::PvcUsed && row.from_ms >= 60_000 && row.to_ms <= 180_000
            })
            .collect::<Vec<_>>();
        assert_eq!(gap.len(), 3);
        assert_eq!(gap[0].measurement_status, MeasurementStatus::Pending);
        assert_eq!(gap[1].measurement_status, middle);
        assert_eq!(gap[2].measurement_status, MeasurementStatus::Pending);
        assert!(gap.iter().all(|row| row.quantity.is_none()));
    };
    check(
        settle(&raw, &interrupted, 0, 240_000, 180_000 + LATE_GRACE_MS),
        MeasurementStatus::NotAllocated,
    );
    let mut other = home.clone();
    other.id = "second-home".into();
    other.pvc_uid = "second-pvc".into();
    raw.extend([
        sample(&other, SourceMetric::PvcUsed, 90_000, 5.0),
        sample(&other, SourceMetric::PvcUsed, 150_000, 5.0),
    ]);
    let continuous = [event(1, 0, 240, disk.clone(), disk)];
    check(
        settle(&raw, &continuous, 0, 240_000, 180_000 + LATE_GRACE_MS),
        MeasurementStatus::Unmapped,
    );
}

#[test]
fn singleton_competing_pvc_observation_refuses_an_interior_estimate() {
    let home = binding(BindingKind::Home);
    let mut other = home.clone();
    other.id = "second-home".into();
    other.pvc_uid = "second-pvc".into();
    let mut raw = [(0, 10.0), (60_000, 10.0), (180_000, 30.0), (240_000, 30.0)]
        .into_iter()
        .map(|(time, value)| sample(&home, SourceMetric::PvcUsed, time, value))
        .collect::<Vec<_>>();
    let events = [event(1, 0, 240, allocation(false), allocation(false))];
    let gap = |rows: Vec<CoverageInterval>| {
        rows.into_iter()
            .find(|row| row.metric == SourceMetric::PvcUsed && row.from_ms == 60_000)
            .unwrap()
    };
    let cutoff = 180_000 + LATE_GRACE_MS;
    assert_eq!(
        gap(settle(&raw, &events, 0, 240_000, cutoff)).measurement_status,
        MeasurementStatus::Estimated
    );
    raw.push(sample(&other, SourceMetric::PvcUsed, 120_000, 5.0));
    let unresolved = gap(settle(&raw, &events, 0, 240_000, cutoff));
    assert_eq!(unresolved.from_ms, 60_000);
    assert_eq!(unresolved.to_ms, 180_000);
    assert_eq!(unresolved.measurement_status, MeasurementStatus::Pending);
    assert_eq!(unresolved.quantity, None);
}

#[test]
fn later_foreign_pvc_observations_cannot_change_the_original_owners_estimate() {
    let primary = binding(BindingKind::Home);
    let mut former = primary.clone();
    former.id = "former-home".into();
    former.pvc_uid = "transferred-pvc".into();
    former.valid_until = Some(50);
    former.observed_at = 50;
    let mut raw = [
        (300_000, 10.0),
        (360_000, 10.0),
        (480_000, 30.0),
        (540_000, 30.0),
    ]
    .into_iter()
    .map(|(time, value)| sample(&primary, SourceMetric::PvcUsed, time, value))
    .collect::<Vec<_>>();
    raw.push(sample(&former, SourceMetric::PvcUsed, 20_000, 5.0));
    let events = [event(1, 0, 540, allocation(false), allocation(false))];
    let cutoff = 480_000 + LATE_GRACE_MS;
    let baseline = settle(&raw, &events, 300_000, 540_000, cutoff);
    assert_eq!(
        baseline
            .iter()
            .find(|row| row.metric == SourceMetric::PvcUsed && row.from_ms == 360_000)
            .unwrap()
            .measurement_status,
        MeasurementStatus::Estimated
    );
    for owner in ["owner-b", "owner-a"] {
        let mut foreign = former.clone();
        foreign.id = format!("later-home-{owner}");
        foreign.owner_subject = owner.into();
        foreign.sandbox_id = "another-sandbox".into();
        foreign.valid_from = 50;
        foreign.valid_until = Some(600);
        foreign.observed_at = 600;
        raw.push(sample(&foreign, SourceMetric::PvcUsed, 100_000, 5.0));
        assert_eq!(settle(&raw, &events, 300_000, 540_000, cutoff), baseline);
        raw.push(sample(&foreign, SourceMetric::PvcUsed, 420_000, 5.0));
        assert_eq!(settle(&raw, &events, 300_000, 540_000, cutoff), baseline);
        raw.truncate(raw.len() - 2);
    }
}
