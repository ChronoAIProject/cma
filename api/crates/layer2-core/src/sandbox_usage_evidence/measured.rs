use super::{measured_model::*, quantize::scaled_half_even};
use crate::sandbox_metrics::{aggregate, Aggregate, Binding, BindingKind, ProofStatus, Range};
use layer3_adapter::sandbox_metrics::PhysicalSeries;
use std::collections::BTreeMap;

const MAX_QUANTITY: u128 = 10_u128.pow(38) - 1;
const MAX_BINDINGS: usize = 64;
const MAX_SAMPLES: usize = 8192;

pub fn freeze_measured(
    mut expected: ExpectedMinute,
    bindings: Vec<Binding>,
    mut series: Vec<PhysicalSeries>,
    source_complete: bool,
) -> Result<FrozenMeasuredMinute, MeasuredError> {
    if expected.sandbox_id.is_empty()
        || expected.owner.is_empty()
        || !expected.bucket_start.is_multiple_of(60)
        || expected.bucket_start > i64::MAX as u64 / 1000 - 60
        || bindings.len() > MAX_BINDINGS
        || series.len() > 64
        || series.iter().map(|s| s.samples.len()).sum::<usize>() > MAX_SAMPLES
    {
        return Err(MeasuredError::Bounds);
    }
    expected.compute = intervals(&expected.compute, expected.bucket_start)?;
    expected.home = intervals(&expected.home, expected.bucket_start)?;
    let bindings = validate_bindings(bindings, &expected)?;
    series.sort_by(|a, b| (&a.metric, &a.name, &a.labels).cmp(&(&b.metric, &b.name, &b.labels)));
    for value in &series {
        if value
            .samples
            .windows(2)
            .any(|pair| pair[0].at >= pair[1].at)
        {
            return Err(MeasuredError::Invalid);
        }
    }
    for pair in series.windows(2) {
        if pair[0].metric == pair[1].metric
            && pair[0].name == pair[1].name
            && pair[0].labels == pair[1].labels
            && pair[0] != pair[1]
        {
            return Err(MeasuredError::Invalid);
        }
    }
    series.dedup();
    let mut totals: [Aggregate; 3] = Default::default();
    let mut unknown = [0.0; 3];
    for (kind, ranges) in [
        (BindingKind::Compute, &expected.compute),
        (BindingKind::Home, &expected.home),
    ] {
        let own: Vec<_> = bindings
            .iter()
            .filter(|b| {
                b.kind == kind
                    && b.sandbox_id == expected.sandbox_id
                    && b.owner_subject == expected.owner
            })
            .cloned()
            .collect();
        let physical: Vec<_> = own
            .iter()
            .filter(|b| b.proof_status == ProofStatus::Verified)
            .map(|b| {
                b.range(expected.bucket_start + 60).intersect(Range {
                    from: expected.bucket_start,
                    to: expected.bucket_start + 60,
                })
            })
            .filter(|r| r.seconds() > 0)
            .collect();
        let mut integration = ranges.clone();
        integration.extend(physical);
        let integration = intervals(&integration, expected.bucket_start)?;
        let mut proven = Vec::new();
        for range in ranges {
            for binding in own
                .iter()
                .filter(|b| b.proof_status == ProofStatus::Verified)
            {
                let valid = binding.range(range.to).intersect(*range);
                if valid.seconds() > 0 {
                    proven.push(valid);
                }
            }
        }
        for range in &integration {
            let values = aggregate::aggregate(&own, &series, *range)
                .map_err(|_| MeasuredError::Attribution)?;
            for (total, value) in totals.iter_mut().zip(aggregate::totals(&values)) {
                total.merge(&value);
            }
        }
        let known = duration(&intervals(&proven, expected.bucket_start)?);
        let missing = duration(ranges)
            .checked_sub(known)
            .ok_or(MeasuredError::Invalid)?;
        for index in if kind == BindingKind::Home {
            &[2][..]
        } else {
            &[0, 1][..]
        } {
            unknown[*index] = missing as f64;
            totals[*index].expected_seconds += missing as f64;
        }
    }
    let mut bits = Vec::new();
    let mut metrics = Vec::new();
    for (index, value) in totals.iter().enumerate() {
        bits.push([
            ExactFloat::new(value.integral)?,
            ExactFloat::new(value.covered_seconds)?,
            ExactFloat::new(value.expected_seconds)?,
        ]);
        metrics.push(metric(value, index, unknown[index] > 0.0, source_complete)?);
    }
    let fact = MeasuredMinute {
        bucket_start: expected.bucket_start,
        compute_uptime_ms: duration(&expected.compute) * 1000,
        retained_home_ms: duration(&expected.home) * 1000,
        metrics: metrics.try_into().map_err(|_| MeasuredError::Invalid)?,
    };
    Ok(FrozenMeasuredMinute {
        version: 1,
        algorithm: "binding-integral-half-even-v1".into(),
        expected,
        bindings: bindings
            .into_iter()
            .map(FrozenBinding::new)
            .collect::<Result<_, _>>()?,
        series: series
            .into_iter()
            .map(FrozenSeries::new)
            .collect::<Result<_, _>>()?,
        source_complete,
        result_bits: bits.try_into().map_err(|_| MeasuredError::Invalid)?,
        fact,
    })
}

fn metric(
    value: &Aggregate,
    index: usize,
    unbound: bool,
    complete: bool,
) -> Result<MeasuredMetric, MeasuredError> {
    let encode = |value, scale| {
        scaled_half_even(value, scale)
            .filter(|quantity| *quantity <= MAX_QUANTITY)
            .ok_or(MeasuredError::Encoding)
    };
    if value.covered_seconds > value.expected_seconds {
        return Err(MeasuredError::Invalid);
    }
    let expected = encode(value.expected_seconds, 1000)?;
    let covered = if complete {
        encode(value.covered_seconds, 1000)?
    } else {
        0
    };
    let quantity = encode(
        value.integral,
        if index == 0 { 1_000_000_000 } else { 1000 },
    )?;
    let (quality, reason, quantity) = if expected == 0 {
        if quantity != 0 || covered != 0 {
            return Err(MeasuredError::Invalid);
        }
        (MeasuredQuality::Observed, None, Some(0))
    } else if !complete || value.covered_seconds == 0.0 {
        let reason = if !complete {
            "source_unavailable"
        } else if unbound {
            "binding_missing"
        } else {
            "sample_unavailable"
        };
        (MeasuredQuality::Missing, Some(reason), None)
    } else if value.covered_seconds == value.expected_seconds {
        (MeasuredQuality::Observed, None, Some(quantity))
    } else if covered == 0 || covered == expected {
        // Millisecond rounding must not promote incomplete physical coverage.
        (
            MeasuredQuality::Estimated,
            Some("coverage_quantized"),
            Some(quantity),
        )
    } else {
        (
            MeasuredQuality::Partial,
            Some(if unbound {
                "binding_missing"
            } else {
                "sample_gap"
            }),
            Some(quantity),
        )
    };
    Ok(MeasuredMetric {
        quantity: quantity.map(|v| v.to_string()),
        covered_binding_ms: covered.to_string(),
        expected_binding_ms: expected.to_string(),
        quality,
        missing_reason: reason.map(str::to_owned),
    })
}

fn validate_bindings(
    bindings: Vec<Binding>,
    expected: &ExpectedMinute,
) -> Result<Vec<Binding>, MeasuredError> {
    let mut unique = BTreeMap::new();
    for binding in bindings {
        if binding.id.is_empty()
            || binding.revision == 0
            || binding.schema_version != 1
            || binding.proof_version != 1
            || binding.valid_until.is_some_and(|v| v < binding.valid_from)
        {
            return Err(MeasuredError::Invalid);
        }
        if binding.sandbox_id == expected.sandbox_id && binding.owner_subject != expected.owner {
            return Err(MeasuredError::Attribution);
        }
        if let Some(previous) = unique.insert(binding.id.clone(), binding.clone()) {
            if previous != binding {
                return Err(MeasuredError::Attribution);
            }
        }
    }
    let values: Vec<Binding> = unique.into_values().collect();
    let minute = Range {
        from: expected.bucket_start,
        to: expected.bucket_start + 60,
    };
    for own in values
        .iter()
        .filter(|b| b.sandbox_id == expected.sandbox_id)
    {
        for other in &values {
            if own.id != other.id
                && own.physical_key() == other.physical_key()
                && own
                    .range(minute.to)
                    .intersect(other.range(minute.to))
                    .intersect(minute)
                    .seconds()
                    > 0
            {
                return Err(MeasuredError::Attribution);
            }
        }
    }
    Ok(values)
}

fn intervals(ranges: &[Range], minute: u64) -> Result<Vec<Range>, MeasuredError> {
    if ranges.len() > 128 {
        return Err(MeasuredError::Bounds);
    }
    let mut ranges = ranges.to_vec();
    if ranges
        .iter()
        .any(|r| r.from < minute || r.to > minute + 60 || r.from >= r.to)
    {
        return Err(MeasuredError::Invalid);
    }
    ranges.sort_by_key(|r| (r.from, r.to));
    let mut merged: Vec<Range> = Vec::new();
    for range in ranges {
        if let Some(last) = merged.last_mut().filter(|last| range.from <= last.to) {
            last.to = last.to.max(range.to);
        } else {
            merged.push(range);
        }
    }
    Ok(merged)
}

fn duration(ranges: &[Range]) -> u64 {
    ranges.iter().map(|range| range.seconds()).sum()
}
