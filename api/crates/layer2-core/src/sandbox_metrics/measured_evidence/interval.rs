//! Exact quantities between consecutive, undisputed observations.
use super::{digest, millisecond_bounds, ObservationV1, RetainedObservation};
use layer3_adapter::sandbox_metrics::SourceMetric;

/// Version 1 uses adjacent samples at most 90 seconds apart, with a left-hand
/// hold for byte gauges.
pub const POLICY_VERSION: u32 = 1;
const MAX_NORMAL_SPACING_MS: u64 = 90_000;

pub enum CustodySample<'a> {
    Undisputed(&'a RetainedObservation),
    Disputed(&'a RetainedObservation),
}

impl CustodySample<'_> {
    pub(super) fn observation(&self) -> &ObservationV1 {
        match self {
            Self::Undisputed(observation) | Self::Disputed(observation) => &observation.0,
        }
    }
}

/// A CPU delta measured in core-seconds, expressed as an integer times a
/// power of two. Provider settlement can sum before rounding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CpuSeconds {
    pub significand: u128,
    pub exponent: i16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quantity {
    Cpu(CpuSeconds),
    ByteMilliseconds(u128),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MeasuredInterval {
    pub policy_version: u32,
    pub metric: SourceMetric,
    pub from_ms: u64,
    pub to_ms: u64,
    pub quantity: Result<Quantity, Unresolved>,
    pub evidence_sha256: [String; 2],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Unresolved {
    #[error("retained observations do not establish one continuous attribution")]
    Attribution,
    #[error("observation interval has no supported measured coverage")]
    Coverage,
    #[error("CPU counter reset between observations")]
    CounterReset,
    #[error("source quantity is outside the exact numeric representation")]
    Precision,
    #[error("physical sample has competing evidence")]
    Disputed,
}

/// Normalize each adjoining pair in one complete physical series from a single
/// custody snapshot. A disputed sample occupies its original position.
pub fn normalize_series(
    samples: &[CustodySample<'_>],
) -> Result<Vec<MeasuredInterval>, Unresolved> {
    let mut ordered: Vec<_> = samples.iter().collect();
    ordered.sort_by_key(|sample| sample.observation().timestamp_ms);
    let Some(first) = ordered.first().map(|sample| sample.observation()) else {
        return Ok(Vec::new());
    };
    for sample in &ordered[1..] {
        if !first
            .shares_physical_series(sample.observation())
            .map_err(|_| Unresolved::Attribution)?
        {
            return Err(Unresolved::Attribution);
        }
    }
    if ordered
        .windows(2)
        .any(|pair| pair[0].observation().timestamp_ms == pair[1].observation().timestamp_ms)
    {
        return Err(Unresolved::Coverage);
    }
    ordered
        .windows(2)
        .map(|pair| interval(pair[0], pair[1]))
        .collect()
}

fn interval(
    first: &CustodySample<'_>,
    next: &CustodySample<'_>,
) -> Result<MeasuredInterval, Unresolved> {
    let (a, b) = (first.observation(), next.observation());
    let quantity = match (first, next) {
        (CustodySample::Undisputed(_), CustodySample::Undisputed(_)) => normalize_pair(a, b),
        _ => Err(Unresolved::Disputed),
    };
    Ok(MeasuredInterval {
        policy_version: POLICY_VERSION,
        metric: a.metric,
        from_ms: a.timestamp_ms,
        to_ms: b.timestamp_ms,
        quantity,
        evidence_sha256: [
            digest(a).map_err(|_| Unresolved::Attribution)?,
            digest(b).map_err(|_| Unresolved::Attribution)?,
        ],
    })
}

fn normalize_pair(first: &ObservationV1, next: &ObservationV1) -> Result<Quantity, Unresolved> {
    verify_continuity(first, next)?;
    let a = f64::from_be_bytes(first.source_value_be);
    let b = f64::from_be_bytes(next.source_value_be);
    if first.metric == SourceMetric::CpuCounter && b < a {
        return Err(Unresolved::CounterReset);
    }
    let elapsed = next
        .timestamp_ms
        .checked_sub(first.timestamp_ms)
        .filter(|elapsed| (1..=MAX_NORMAL_SPACING_MS).contains(elapsed))
        .ok_or(Unresolved::Coverage)?;
    let quantity = match first.metric {
        SourceMetric::CpuCounter => Quantity::Cpu(cpu_delta(a, b)?),
        SourceMetric::WorkingSet | SourceMetric::PvcUsed | SourceMetric::PvcCapacity => {
            let bytes = u128::from(exact_bytes(a)?);
            exact_bytes(b)?;
            Quantity::ByteMilliseconds(bytes * u128::from(elapsed))
        }
    };
    Ok(quantity)
}

fn verify_continuity(first: &ObservationV1, next: &ObservationV1) -> Result<(), Unresolved> {
    let a = &first.binding;
    let b = &next.binding;
    let proof = match a.revision.cmp(&b.revision) {
        std::cmp::Ordering::Greater => a,
        std::cmp::Ordering::Less => b,
        std::cmp::Ordering::Equal => {
            if digest(a).map_err(|_| Unresolved::Attribution)?
                != digest(b).map_err(|_| Unresolved::Attribution)?
            {
                return Err(Unresolved::Attribution);
            }
            b
        }
    };
    let (from, to) =
        millisecond_bounds(proof.range(u64::MAX)).map_err(|_| Unresolved::Attribution)?;
    if a.id != b.id
        || a.owner_subject != b.owner_subject
        || a.sandbox_id != b.sandbox_id
        || (!first.metric.is_pvc()
            && (a.runtime_generation, &a.native_sandbox_uid, &a.claim_uid)
                != (b.runtime_generation, &b.native_sandbox_uid, &b.claim_uid))
        || !(from..to).contains(&first.timestamp_ms)
        || !(from..to).contains(&next.timestamp_ms)
    {
        return Err(Unresolved::Attribution);
    }
    Ok(())
}

fn exact_bytes(value: f64) -> Result<u64, Unresolved> {
    if value.fract() == 0.0 && value < (1_u128 << 64) as f64 {
        Ok(value as u64)
    } else {
        Err(Unresolved::Precision)
    }
}

fn cpu_delta(a: f64, b: f64) -> Result<CpuSeconds, Unresolved> {
    let (first, first_exponent) = cpu_parts(a);
    let (next, next_exponent) = cpu_parts(b);
    let exponent = if first == 0 {
        next_exponent
    } else {
        first_exponent.min(next_exponent)
    };
    let (first_high, first_low) = align_cpu(first, first_exponent - exponent)?;
    let (next_high, next_low) = align_cpu(next, next_exponent - exponent)?;
    let (delta, borrowed) = next_low.overflowing_sub(first_low);
    // A single high bit may be consumed by the subtraction. If it survives,
    // the aligned result cannot fit. Different canonical exponents yield an
    // odd difference, and equal exponents never need a high bit.
    if next_high - first_high - i32::from(borrowed) != 0 {
        return Err(Unresolved::Precision);
    }
    if delta == 0 {
        return Ok(CpuSeconds {
            significand: 0,
            exponent: 0,
        });
    }
    let trailing = delta.trailing_zeros();
    Ok(CpuSeconds {
        significand: delta >> trailing,
        exponent: exponent + trailing as i16,
    })
}

fn align_cpu(significand: u64, shift: i16) -> Result<(i32, u128), Unresolved> {
    if significand == 0 {
        return Ok((0, 0));
    }
    let shift = u32::try_from(shift).map_err(|_| Unresolved::Precision)?;
    let width = u64::BITS - significand.leading_zeros() + shift;
    if width > u128::BITS + 1 {
        return Err(Unresolved::Precision);
    }
    let low = u128::from(significand).checked_shl(shift).unwrap_or(0);
    Ok((i32::from(width > u128::BITS), low))
}

fn cpu_parts(value: f64) -> (u64, i16) {
    let bits = value.to_bits();
    let mantissa = bits & ((1 << 52) - 1);
    let encoded_exponent = ((bits >> 52) & 0x7ff) as i16;
    let (significand, exponent) = if encoded_exponent == 0 {
        (mantissa, -1074)
    } else {
        (mantissa | (1 << 52), encoded_exponent - 1075)
    };
    if significand == 0 {
        return (0, 0);
    }
    let trailing = significand.trailing_zeros();
    (significand >> trailing, exponent + trailing as i16)
}
