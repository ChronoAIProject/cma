//! Allocation-correlated measured usage derived from complete custody snapshots.
use super::{
    digest,
    interval::{normalize_series, CustodySample, MeasuredInterval, Unresolved},
};
use crate::sandbox_usage::UsageEvent;
use layer3_adapter::sandbox_metrics::SourceMetric;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const COVERAGE_POLICY_VERSION: u32 = 1;
pub const LATE_GRACE_MS: u64 = 15 * 60 * 1000;
pub const MAX_INTERIOR_GAP_MS: u64 = 5 * 60 * 1000;
// Both boundary neighbors and their normal-spacing evidence remain in scope.
pub const SUPPORT_WINDOW_MS: u64 = 10 * 60 * 1000;
const BILLABLE_METRICS: [SourceMetric; 3] = [
    SourceMetric::CpuCounter,
    SourceMetric::WorkingSet,
    SourceMetric::PvcUsed,
];
#[path = "coverage_quantity.rs"]
mod quantity;
pub use quantity::ExactQuantity;
use quantity::{allocation_quantity, exact, whole, Exact};
#[path = "coverage_estimation.rs"]
mod estimation;
use estimation::{continuously_allocated, estimate_interior};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeasurementStatus {
    Measured,
    Estimated,
    Pending,
    Unmapped,
    NotRunning,
    NotAllocated,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoverageReason {
    MissingAllocation,
    NoObservation,
    NoUsablePvcSample,
    AttributionUnresolved,
    CounterReset,
    Precision,
    SamplingGap,
    MultiplePhysicalSeries,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuantityDerivation {
    CounterDelta,
    ProportionalCounterDelta,
    LeftHold,
    NeighboringRates,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceReference {
    pub physical_sample_id: String,
    pub evidence_sha256: String,
    pub timestamp_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoverageInterval {
    pub metric: SourceMetric,
    pub from_ms: u64,
    pub to_ms: u64,
    pub measurement_status: MeasurementStatus,
    pub reason: Option<CoverageReason>,
    pub derivation: Option<QuantityDerivation>,
    pub expected_ms: u64,
    pub allocation_revision: Option<u64>,
    pub allocation_sha256: Option<String>,
    pub allocated_quantity: Option<ExactQuantity>,
    /// Core-seconds for CPU and byte-milliseconds for RAM and PVC.
    pub quantity: Option<ExactQuantity>,
    pub physical_series_sha256: Option<String>,
    pub runtime_generation: Option<u64>,
    pub source_references: Vec<SourceReference>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum CoverageError {
    #[error("allocation history is inconsistent with the requested owner and Sandbox")]
    Allocation,
    #[error("measured custody cannot establish an exhaustive physical series")]
    Custody,
    #[error("measured settlement window is invalid")]
    Window,
}

struct AllocationSpan<'a> {
    from_ms: u64,
    to_ms: u64,
    event: &'a UsageEvent,
    digest: String,
}

struct ObservedSeries {
    series: String,
    metric: SourceMetric,
    timestamps_ms: Vec<u64>,
}

struct Candidate {
    interval: MeasuredInterval,
    series: String,
    binding: String,
    attributed: bool,
    estimate: Option<Exact>,
    references: Vec<SourceReference>,
    runtime_generation: Option<u64>,
}

/// Produce an ordered half-open partition for each billable metric. Missing
/// authoritative allocation never establishes zero or a measured amount.
pub fn normalize_coverage(
    owner: &str,
    sandbox: &str,
    samples: &[CustodySample<'_>],
    allocation_events: &[UsageEvent],
    from_ms: u64,
    to_ms: u64,
    cutoff_ms: u64,
) -> Result<Vec<CoverageInterval>, CoverageError> {
    if owner.is_empty() || sandbox.is_empty() || from_ms >= to_ms || to_ms > cutoff_ms {
        return Err(CoverageError::Window);
    }
    let allocation = allocation_spans(owner, sandbox, allocation_events)?;
    let candidates = candidates(
        owner,
        sandbox,
        samples,
        &allocation,
        from_ms.saturating_sub(SUPPORT_WINDOW_MS)
            ..=to_ms.saturating_add(SUPPORT_WINDOW_MS).min(cutoff_ms),
        cutoff_ms,
    )?;
    let mut intervals = Vec::new();
    for metric in BILLABLE_METRICS {
        let selected: Vec<_> = candidates
            .iter()
            .filter(|value| value.interval.metric == metric)
            .collect();
        let mut boundaries = BTreeSet::from([from_ms, to_ms]);
        for (start, end) in allocation
            .iter()
            .map(|span| (span.from_ms, span.to_ms))
            .chain(
                selected
                    .iter()
                    .map(|value| (value.interval.from_ms, value.interval.to_ms)),
            )
        {
            if start < to_ms && from_ms < end {
                boundaries.insert(start.max(from_ms));
                boundaries.insert(end.min(to_ms));
            }
        }
        let boundaries: Vec<_> = boundaries.into_iter().collect();
        for pair in boundaries.windows(2) {
            let (start, end) = (pair[0], pair[1]);
            let span = allocation
                .iter()
                .find(|span| span.from_ms <= start && end <= span.to_ms);
            let supporting: Vec<_> = selected
                .iter()
                .copied()
                .filter(|value| value.interval.from_ms <= start && end <= value.interval.to_ms)
                .collect();
            intervals.push(classify(metric, start, end, span, &supporting));
        }
    }
    Ok(intervals)
}

fn allocation_spans<'a>(
    owner: &str,
    sandbox: &str,
    events: &'a [UsageEvent],
) -> Result<Vec<AllocationSpan<'a>>, CoverageError> {
    let mut sorted: Vec<_> = events.iter().collect();
    sorted.sort_by_key(|event| event.revision);
    let mut spans = Vec::new();
    let mut previous: Option<&UsageEvent> = None;
    for event in sorted {
        if event.owner.as_str() != owner
            || event.sandbox_id.as_str() != sandbox
            || event.revision == 0
            || event.started_at > event.ended_at
            || event
                .allocation
                .usage(event.ended_at - event.started_at)
                .ok()
                .as_ref()
                != Some(&event.quantity)
        {
            return Err(CoverageError::Allocation);
        }
        if let Some(prior) = previous {
            if prior.revision.checked_add(1) != Some(event.revision)
                || prior.ended_at != event.started_at
                || prior.next_allocation != event.allocation
                || prior.source_seq > event.source_seq
            {
                return Err(CoverageError::Allocation);
            }
        }
        previous = Some(event);
        if event.started_at != event.ended_at {
            spans.push(AllocationSpan {
                from_ms: event
                    .started_at
                    .checked_mul(1000)
                    .ok_or(CoverageError::Allocation)?,
                to_ms: event
                    .ended_at
                    .checked_mul(1000)
                    .ok_or(CoverageError::Allocation)?,
                event,
                digest: digest(event).map_err(|_| CoverageError::Allocation)?,
            });
        }
    }
    Ok(spans)
}

fn candidates(
    owner: &str,
    sandbox: &str,
    samples: &[CustodySample<'_>],
    allocation: &[AllocationSpan<'_>],
    support: std::ops::RangeInclusive<u64>,
    cutoff_ms: u64,
) -> Result<Vec<Candidate>, CoverageError> {
    let mut grouped = BTreeMap::<String, Vec<&CustodySample<'_>>>::new();
    for sample in samples {
        let observation = sample.observation();
        if !support.contains(&observation.timestamp_ms) {
            continue;
        }
        let series = observation
            .series_id()
            .map_err(|_| CoverageError::Custody)?;
        grouped.entry(series).or_default().push(sample);
    }
    let mut result = Vec::new();
    let mut observed_series = Vec::new();
    for (series, mut group) in grouped {
        group.sort_by_key(|sample| sample.observation().timestamp_ms);
        let owned = |sample: &CustodySample<'_>| {
            let binding = &sample.observation().binding;
            binding.owner_subject == owner && binding.sandbox_id == sandbox
        };
        if !group.iter().any(|sample| owned(sample)) {
            continue;
        }
        observed_series.push(ObservedSeries {
            series: series.clone(),
            metric: group[0].observation().metric,
            timestamps_ms: group
                .iter()
                .filter(|sample| owned(sample))
                .map(|sample| sample.observation().timestamp_ms)
                .collect(),
        });
        let ordered: Vec<_> = group
            .iter()
            .map(|sample| match sample {
                CustodySample::Undisputed(value) => CustodySample::Undisputed(value),
                CustodySample::Disputed(value) => CustodySample::Disputed(value),
            })
            .collect();
        let normal = normalize_series(&ordered).map_err(|_| CoverageError::Custody)?;
        let mut series_candidates: Vec<_> = normal
            .into_iter()
            .zip(group.windows(2))
            .filter_map(|(interval, pair)| {
                if !owned(pair[0]) && !owned(pair[1]) {
                    return None;
                }
                let a = &pair[0].observation().binding;
                let b = &pair[1].observation().binding;
                let attributed = owned(pair[0])
                    && owned(pair[1])
                    && a.id == b.id
                    && interval.quantity != Err(Unresolved::Attribution)
                    && interval.quantity != Err(Unresolved::Disputed);
                Some(Candidate {
                    references: pair
                        .iter()
                        .zip(&interval.evidence_sha256)
                        .map(|(sample, hash)| SourceReference {
                            physical_sample_id: sample.observation().physical_sample_id.clone(),
                            evidence_sha256: hash.clone(),
                            timestamp_ms: sample.observation().timestamp_ms,
                        })
                        .collect(),
                    runtime_generation: (!interval.metric.is_pvc())
                        .then_some(a.runtime_generation)
                        .flatten(),
                    interval,
                    series: series.clone(),
                    binding: a.id.clone(),
                    attributed,
                    estimate: None,
                })
            })
            .collect();
        for candidate in &mut series_candidates {
            if !candidate.interval.metric.is_pvc() {
                candidate.attributed &= continuously_allocated(allocation, candidate);
            }
        }
        result.extend(series_candidates);
    }
    estimate_interior(&mut result, &observed_series, cutoff_ms, allocation);
    Ok(result)
}

fn classify(
    metric: SourceMetric,
    from_ms: u64,
    to_ms: u64,
    allocation: Option<&AllocationSpan<'_>>,
    candidates: &[&Candidate],
) -> CoverageInterval {
    let duration = to_ms - from_ms;
    let mut row = CoverageInterval {
        metric,
        from_ms,
        to_ms,
        measurement_status: MeasurementStatus::Unmapped,
        reason: Some(CoverageReason::MissingAllocation),
        derivation: None,
        expected_ms: 0,
        allocation_revision: None,
        allocation_sha256: None,
        allocated_quantity: None,
        quantity: None,
        physical_series_sha256: None,
        runtime_generation: None,
        source_references: Vec::new(),
    };
    let Some(allocation) = allocation else {
        return row;
    };
    row.allocation_revision = Some(allocation.event.revision);
    row.allocation_sha256 = Some(allocation.digest.clone());
    let cap = allocation_quantity(metric, &allocation.event.allocation, duration);
    row.allocated_quantity = Some(cap.clone().into());
    row.reason = None;
    if !metric.is_pvc() && !allocation.event.allocation.running {
        row.measurement_status = MeasurementStatus::NotRunning;
        return row;
    }
    if metric.is_pvc() && allocation.event.allocation.disk_gib == 0 {
        row.measurement_status = MeasurementStatus::NotAllocated;
        return row;
    }
    row.expected_ms = duration;
    row.measurement_status = MeasurementStatus::Pending;
    row.reason = Some(if metric.is_pvc() {
        CoverageReason::NoUsablePvcSample
    } else {
        CoverageReason::NoObservation
    });
    match candidates {
        [] => row,
        [candidate] => {
            if !candidate.attributed {
                row.measurement_status = MeasurementStatus::Unmapped;
                row.reason = Some(CoverageReason::AttributionUnresolved);
                return row;
            }
            row.physical_series_sha256 = Some(candidate.series.clone());
            row.runtime_generation = candidate.runtime_generation;
            row.source_references.clone_from(&candidate.references);
            let part =
                whole(duration) / whole(candidate.interval.to_ms - candidate.interval.from_ms);
            if let Ok(quantity) = candidate.interval.quantity {
                row.reason = None;
                let proportional = metric == SourceMetric::CpuCounter
                    && (from_ms != candidate.interval.from_ms || to_ms != candidate.interval.to_ms);
                let quantity = exact(quantity) * part;
                row.derivation = Some(match (metric, proportional) {
                    (SourceMetric::CpuCounter, true) => {
                        QuantityDerivation::ProportionalCounterDelta
                    }
                    (SourceMetric::CpuCounter, false) => QuantityDerivation::CounterDelta,
                    _ => QuantityDerivation::LeftHold,
                });
                if proportional {
                    row.quantity = Some(quantity.min(cap).into());
                    row.measurement_status = MeasurementStatus::Estimated;
                } else {
                    row.quantity = Some(quantity.into());
                    row.measurement_status = MeasurementStatus::Measured;
                }
            } else if let Some(estimate) = &candidate.estimate {
                row.reason = None;
                row.quantity = Some((estimate.clone() * part).min(cap).into());
                row.measurement_status = MeasurementStatus::Estimated;
                row.derivation = Some(QuantityDerivation::NeighboringRates);
            } else {
                row.reason = Some(match candidate.interval.quantity {
                    Err(Unresolved::CounterReset) => CoverageReason::CounterReset,
                    Err(Unresolved::Precision) => CoverageReason::Precision,
                    _ => CoverageReason::SamplingGap,
                });
            }
            row
        }
        _ => {
            row.measurement_status = MeasurementStatus::Unmapped;
            row.reason = Some(CoverageReason::MultiplePhysicalSeries);
            row
        }
    }
}

#[cfg(test)]
#[path = "coverage_tests.rs"]
mod tests;
