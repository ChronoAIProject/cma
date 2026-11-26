//! Reset-aware counter deltas and bounded gauge holds, before any aggregation.
use super::{
    model::option_max, model::option_min, Aggregate, Binding, BindingKind, Metric, MetricsError,
    Minute, ProofStatus, Range, Result,
};
use layer3_adapter::sandbox_metrics::{PhysicalSeries, Sample, SourceMetric};
use std::collections::{BTreeMap, BTreeSet};

pub fn aggregate(
    bindings: &[Binding],
    series: &[PhysicalSeries],
    range: Range,
) -> Result<Vec<Minute>> {
    aggregate_observing(bindings, series, range, &mut |_, _, _, _| {})
}

pub(crate) fn aggregate_observing(
    bindings: &[Binding],
    series: &[PhysicalSeries],
    range: Range,
    observe: &mut impl FnMut(&Binding, Metric, f64, f64),
) -> Result<Vec<Minute>> {
    if range.seconds() > 86_400 {
        return Err(MetricsError::Invalid(
            "aggregate chunk exceeds one day".into(),
        ));
    }
    let mut minutes = BTreeMap::new();
    let mut physical = BTreeSet::new();
    for binding in bindings {
        if binding.proof_status != ProofStatus::Verified {
            continue;
        }
        let supported = binding.range(range.to).intersect(range);
        if supported.seconds() == 0 {
            continue;
        }
        // Never multiply one home by Agent count, mount count or a duplicate
        // attachment. Contradictory overlap is rejected, not arbitrarily chosen.
        let physical_id = binding.physical_key();
        for other in bindings
            .iter()
            .filter(|b| b.id != binding.id && b.physical_key() == physical_id)
        {
            if other.range(range.to).intersect(supported).seconds() > 0 {
                return Err(MetricsError::Unavailable);
            }
        }
        if !physical.insert(binding.id.clone()) {
            continue;
        }
        for metric in if binding.kind == BindingKind::Home {
            &[Metric::Disk][..]
        } else {
            &[Metric::Cpu, Metric::Memory][..]
        } {
            visit(
                &mut minutes,
                supported.from as f64,
                supported.to as f64,
                *metric,
                |v, seconds| v.expected_seconds += seconds,
            );
            let source = match metric {
                Metric::Cpu => SourceMetric::CpuCounter,
                Metric::Memory => SourceMetric::WorkingSet,
                Metric::Disk => SourceMetric::PvcUsed,
            };
            let name = if *metric == Metric::Disk {
                Some(binding.pvc_name.as_str())
            } else {
                binding.pod_name.as_deref()
            };
            let mut matching = series.iter().filter(|s| {
                s.metric == source
                    && Some(s.name.as_str()) == name
                    && s.labels.get("cluster") == Some(&binding.cluster)
                    && s.labels.get("namespace") == Some(&binding.namespace)
            });
            let Some(samples) = matching.next() else {
                continue;
            };
            if matching.next().is_some() {
                return Err(MetricsError::Unavailable);
            }
            validate_samples(&samples.samples)?;
            let mut capacities = series.iter().filter(|s| {
                s.metric == SourceMetric::PvcCapacity
                    && s.name == binding.pvc_name
                    && s.labels.get("cluster") == Some(&binding.cluster)
                    && s.labels.get("namespace") == Some(&binding.namespace)
            });
            let capacity = capacities.next();
            if capacities.next().is_some() {
                return Err(MetricsError::Unavailable);
            }
            if let Some(capacity) = capacity {
                validate_samples(&capacity.samples)?;
            }
            let bound = binding.range(u64::MAX);
            let capacity_samples: Vec<_> = capacity
                .into_iter()
                .flat_map(|s| &s.samples)
                .filter(|s| s.at >= bound.from as f64 && s.at < bound.to as f64)
                .copied()
                .collect();
            let samples: Vec<_> = samples
                .samples
                .iter()
                .copied()
                .filter(|s| s.at >= bound.from as f64 && s.at < bound.to as f64)
                .collect();
            let mut observed = |metric, from, to| observe(binding, metric, from, to);
            if *metric == Metric::Cpu {
                counters(
                    &mut minutes,
                    &samples,
                    supported,
                    binding.cpu_limit_cores,
                    &mut observed,
                );
            } else {
                gauges(
                    &mut minutes,
                    &samples,
                    supported,
                    *metric,
                    binding.memory_limit_bytes,
                    &capacity_samples,
                    &mut observed,
                );
            }
        }
    }
    Ok(minutes.into_values().collect())
}
fn validate_samples(samples: &[Sample]) -> Result<()> {
    if samples
        .iter()
        .any(|s| !s.at.is_finite() || !s.value.is_finite() || s.value < 0.0)
        || samples.windows(2).any(|w| w[0].at >= w[1].at)
    {
        Err(MetricsError::Unavailable)
    } else {
        Ok(())
    }
}
fn counters(
    minutes: &mut BTreeMap<u64, Minute>,
    samples: &[Sample],
    range: Range,
    capacity: Option<f64>,
    observe: &mut impl FnMut(Metric, f64, f64),
) {
    let mut baseline = 0;
    for (i, pair) in samples.windows(2).enumerate() {
        let (a, b) = (pair[0], pair[1]);
        let dt = b.at - a.at;
        if dt > 90.0 || b.value < a.value {
            baseline = i + 1;
            continue;
        }
        while baseline < i && samples[baseline].at < b.at - 120.0 {
            baseline += 1;
        }
        let display = (b.value - samples[baseline].value) / (b.at - samples[baseline].at);
        let rate = (b.value - a.value) / dt;
        add(
            minutes,
            Segment {
                from: a.at.max(range.from as f64),
                to: b.at.min(range.to as f64),
                value: rate,
                envelope: display,
                observed_at: b.at,
                capacity,
            },
            Metric::Cpu,
            observe,
        );
    }
}
fn gauges(
    minutes: &mut BTreeMap<u64, Minute>,
    samples: &[Sample],
    range: Range,
    metric: Metric,
    memory_capacity: Option<f64>,
    capacities: &[Sample],
    observe: &mut impl FnMut(Metric, f64, f64),
) {
    for (i, sample) in samples.iter().enumerate() {
        let end = samples
            .get(i + 1)
            .map_or(range.to as f64, |next| next.at)
            .min(sample.at + 90.0)
            .min(range.to as f64);
        let mut from = sample.at.max(range.from as f64);
        while from < end {
            let mut to = end;
            let capacity = if metric == Metric::Disk {
                // Capacity has its own scrape clock. Split at changes and
                // expiry so its denominator covers precisely the same time
                // as the corresponding usage numerator, with no stale hold.
                let next = capacities.partition_point(|s| s.at <= from);
                if let Some(sample) = capacities.get(next) {
                    to = to.min(sample.at);
                }
                next.checked_sub(1).and_then(|i| {
                    let sample = capacities[i];
                    (from < sample.at + 90.0).then(|| {
                        to = to.min(sample.at + 90.0);
                        sample.value
                    })
                })
            } else {
                memory_capacity
            };
            add(
                minutes,
                Segment {
                    from,
                    to,
                    value: sample.value,
                    envelope: sample.value,
                    observed_at: sample.at,
                    capacity,
                },
                metric,
                observe,
            );
            from = to;
        }
    }
}
struct Segment {
    from: f64,
    to: f64,
    value: f64,
    envelope: f64,
    observed_at: f64,
    capacity: Option<f64>,
}
fn add(
    minutes: &mut BTreeMap<u64, Minute>,
    s: Segment,
    metric: Metric,
    observe: &mut impl FnMut(Metric, f64, f64),
) {
    if s.from < s.to {
        observe(metric, s.from, s.to);
    }
    visit(minutes, s.from, s.to, metric, |v, seconds| {
        v.integral += s.value * seconds;
        v.display_integral += s.envelope * seconds;
        v.covered_seconds += seconds;
        if let Some(capacity) = s.capacity.filter(|v| v.is_finite() && *v > 0.0) {
            v.capacity_integral += capacity * seconds;
            v.capacity_usage_integral += s.envelope * seconds;
            v.capacity_covered_seconds += seconds;
        }
        v.min = option_min(v.min, Some(s.envelope));
        v.max = option_max(v.max, Some(s.envelope));
        v.available_from = option_min(v.available_from, Some(s.from));
        if v.observed_at.is_none_or(|at| s.observed_at >= at) {
            v.observed_at = Some(s.observed_at);
            v.latest = Some(s.envelope);
            v.latest_capacity = s.capacity;
        }
    });
}
fn visit(
    minutes: &mut BTreeMap<u64, Minute>,
    from: f64,
    to: f64,
    metric: Metric,
    mut apply: impl FnMut(&mut Aggregate, f64),
) {
    if from >= to {
        return;
    }
    let mut at = (from as u64) / 60 * 60;
    while (at as f64) < to {
        let next = at + 60;
        let seconds = (next as f64).min(to) - (at as f64).max(from);
        let minute = minutes.entry(at).or_insert_with(|| Minute {
            from: at,
            to: next,
            ..Default::default()
        });
        apply(&mut minute.metrics[metric.index()], seconds);
        at = next;
    }
}
pub fn totals(minutes: &[Minute]) -> [Aggregate; 3] {
    let mut result = [
        Aggregate::default(),
        Aggregate::default(),
        Aggregate::default(),
    ];
    for minute in minutes {
        for (total, value) in result.iter_mut().zip(&minute.metrics) {
            total.merge(value);
        }
    }
    result
}
