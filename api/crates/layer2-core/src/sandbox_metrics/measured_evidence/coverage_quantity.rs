use super::super::interval::Quantity;
use crate::sandbox_usage::Allocation;
use layer3_adapter::sandbox_metrics::SourceMetric;
use num_bigint::BigUint;
use num_rational::Ratio;
use serde::{Deserialize, Serialize};

pub(in crate::sandbox_metrics::measured_evidence) type Exact = Ratio<BigUint>;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExactQuantity {
    pub numerator: String,
    pub denominator: String,
}

impl ExactQuantity {
    pub(crate) fn checked_ratio(&self) -> Option<Ratio<BigUint>> {
        fn decimal(value: &str) -> Option<BigUint> {
            if value.len() > 400
                || value.is_empty()
                || (value.len() > 1 && value.starts_with('0'))
                || !value.bytes().all(|byte| byte.is_ascii_digit())
            {
                return None;
            }
            BigUint::parse_bytes(value.as_bytes(), 10)
        }
        let numerator = decimal(&self.numerator)?;
        let denominator = decimal(&self.denominator)?;
        if denominator == BigUint::from(0_u8) {
            return None;
        }
        let ratio = Ratio::new(numerator, denominator);
        if Self::from(ratio.clone()) != *self {
            return None;
        }
        Some(ratio)
    }
}

impl From<Exact> for ExactQuantity {
    fn from(value: Exact) -> Self {
        Self {
            numerator: value.numer().to_str_radix(10),
            denominator: value.denom().to_str_radix(10),
        }
    }
}

pub(super) fn allocation_quantity(
    metric: SourceMetric,
    allocation: &Allocation,
    duration: u64,
) -> Exact {
    match metric {
        SourceMetric::CpuCounter => {
            whole(u64::from(allocation.cpu_millis)) * whole(duration) / whole(1_000_000_u64)
        }
        SourceMetric::WorkingSet => whole(u64::from(allocation.memory_mib) << 20) * whole(duration),
        SourceMetric::PvcUsed | SourceMetric::PvcCapacity => {
            whole(u64::from(allocation.disk_gib) << 30) * whole(duration)
        }
    }
}

pub(super) fn whole(value: impl Into<BigUint>) -> Exact {
    Ratio::from_integer(value.into())
}
pub(super) fn exact(quantity: Quantity) -> Exact {
    match quantity {
        Quantity::ByteMilliseconds(value) => whole(value),
        Quantity::Cpu(value) if value.exponent >= 0 => {
            whole(BigUint::from(value.significand) << value.exponent as usize)
        }
        Quantity::Cpu(value) => Ratio::new(
            BigUint::from(value.significand),
            BigUint::from(1_u8) << usize::from(value.exponent.unsigned_abs()),
        ),
    }
}
