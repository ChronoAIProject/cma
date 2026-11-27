use crate::sandbox_metrics::{Binding, Range};
use layer3_adapter::sandbox_metrics::{PhysicalSeries, Sample, SourceMetric};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectedMinute {
    pub sandbox_id: String,
    pub owner: String,
    pub bucket_start: u64,
    pub compute: Vec<Range>,
    pub home: Vec<Range>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeasuredQuality {
    Observed,
    Partial,
    Missing,
    Estimated,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeasuredMetric {
    pub quantity: Option<String>,
    pub covered_binding_ms: String,
    pub expected_binding_ms: String,
    pub quality: MeasuredQuality,
    pub missing_reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeasuredMinute {
    pub bucket_start: u64,
    pub compute_uptime_ms: u64,
    pub retained_home_ms: u64,
    pub metrics: [MeasuredMetric; 3],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum MeasuredError {
    #[error("invalid measured evidence")]
    Invalid,
    #[error("measured evidence exceeds its input bounds")]
    Bounds,
    #[error("measured evidence attribution conflicts")]
    Attribution,
    #[error("measured evidence cannot be represented exactly")]
    Encoding,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrozenMeasuredMinute {
    pub(super) version: u8,
    pub(super) algorithm: String,
    pub(super) expected: ExpectedMinute,
    pub(super) bindings: Vec<FrozenBinding>,
    pub(super) series: Vec<FrozenSeries>,
    pub(super) source_complete: bool,
    pub(super) result_bits: [[ExactFloat; 3]; 3],
    pub(super) fact: MeasuredMinute,
}

impl FrozenMeasuredMinute {
    pub fn fact(&self) -> &MeasuredMinute {
        &self.fact
    }

    /// Explicit retained-proof verification; delivery reads frozen event bytes.
    pub fn verify_replay(&self) -> Result<(), MeasuredError> {
        if self.recompute_retained()? != *self {
            return Err(MeasuredError::Invalid);
        }
        Ok(())
    }

    /// Explicit operator repair may recalculate retained inputs after a bug fix.
    /// Transport retries continue to use the original frozen event bytes.
    pub fn recompute_retained(&self) -> Result<Self, MeasuredError> {
        if self.version != 1 || self.algorithm != "binding-integral-half-even-v1" {
            return Err(MeasuredError::Invalid);
        }
        let bindings = self
            .bindings
            .iter()
            .map(FrozenBinding::restore)
            .collect::<Result<Vec<_>, _>>()?;
        let series = self.series.iter().map(FrozenSeries::restore).collect();
        super::freeze_measured(
            self.expected.clone(),
            bindings,
            series,
            self.source_complete,
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ExactFloat(u64);

impl ExactFloat {
    pub(super) fn new(value: f64) -> Result<Self, MeasuredError> {
        if !value.is_finite() || value < 0.0 {
            return Err(MeasuredError::Encoding);
        }
        Ok(Self(value.to_bits()))
    }

    fn value(self) -> f64 {
        f64::from_bits(self.0)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FloatRecord {
    value: f64,
    ieee754: String,
}

impl Serialize for ExactFloat {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        FloatRecord {
            value: self.value(),
            ieee754: format!("{:016x}", self.0),
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ExactFloat {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let record = FloatRecord::deserialize(deserializer)?;
        let value = Self::new(record.value).map_err(serde::de::Error::custom)?;
        if record.ieee754 != format!("{:016x}", value.0) {
            return Err(serde::de::Error::custom(
                "measured floating-point bits differ",
            ));
        }
        Ok(value)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct FrozenBinding {
    binding: Binding,
    limit_bits: [Option<ExactFloat>; 2],
}

impl FrozenBinding {
    pub(super) fn new(binding: Binding) -> Result<Self, MeasuredError> {
        let limit_bits = [
            binding.cpu_limit_cores.map(ExactFloat::new).transpose()?,
            binding
                .memory_limit_bytes
                .map(ExactFloat::new)
                .transpose()?,
        ];
        Ok(Self {
            binding,
            limit_bits,
        })
    }

    pub(super) fn restore(&self) -> Result<Binding, MeasuredError> {
        if Self::new(self.binding.clone())?.limit_bits != self.limit_bits {
            return Err(MeasuredError::Invalid);
        }
        Ok(self.binding.clone())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct FrozenSeries {
    metric: SourceMetric,
    name: String,
    labels: BTreeMap<String, String>,
    samples: Vec<[ExactFloat; 2]>,
}

impl FrozenSeries {
    pub(super) fn new(series: PhysicalSeries) -> Result<Self, MeasuredError> {
        Ok(Self {
            metric: series.metric,
            name: series.name,
            labels: series.labels,
            samples: series
                .samples
                .into_iter()
                .map(|s| Ok([ExactFloat::new(s.at)?, ExactFloat::new(s.value)?]))
                .collect::<Result<_, MeasuredError>>()?,
        })
    }

    pub(super) fn restore(&self) -> PhysicalSeries {
        PhysicalSeries {
            metric: self.metric,
            name: self.name.clone(),
            labels: self.labels.clone(),
            samples: self
                .samples
                .iter()
                .map(|[at, value]| Sample {
                    at: at.value(),
                    value: value.value(),
                })
                .collect(),
        }
    }
}
