//! Fixed, bounded monitoring reads; no caller-supplied query language.
mod export;
mod transport;
pub use export::{decode_export, decode_raw_export, selector};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
pub use transport::{MetricsClient, MetricsTransport};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceMetric {
    CpuCounter,
    WorkingSet,
    PvcUsed,
    PvcCapacity,
}
impl SourceMetric {
    pub const ALL: [Self; 4] = [
        Self::CpuCounter,
        Self::WorkingSet,
        Self::PvcUsed,
        Self::PvcCapacity,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Self::CpuCounter => "container_cpu_usage_seconds_total",
            Self::WorkingSet => "container_memory_working_set_bytes",
            Self::PvcUsed => "kubelet_volume_stats_used_bytes",
            Self::PvcCapacity => "kubelet_volume_stats_capacity_bytes",
        }
    }
    pub fn is_pvc(self) -> bool {
        matches!(self, Self::PvcUsed | Self::PvcCapacity)
    }
    pub fn identity_label(self) -> &'static str {
        if self.is_pvc() {
            "persistentvolumeclaim"
        } else {
            "pod"
        }
    }
    pub fn metrics_path(self) -> &'static str {
        if self.is_pvc() {
            "/metrics"
        } else {
            "/metrics/resource"
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sample {
    pub at: f64,
    /// The source can mark a series stale at this timestamp.
    pub value: Option<f64>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PhysicalSeries {
    pub metric: SourceMetric,
    pub name: String,
    pub labels: BTreeMap<String, String>,
    pub samples: Vec<Sample>,
}

pub struct ReportingExport {
    pub series: Vec<PhysicalSeries>,
    pub ambiguous_names: BTreeSet<String>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RawSample {
    pub timestamp_ms: u64,
    pub value: f64,
}

/// Raw values and provenance admitted by the bounded export decoder.
#[derive(Clone, Debug, PartialEq)]
pub struct RawPhysicalSeries {
    metric: SourceMetric,
    name: String,
    labels: BTreeMap<String, String>,
    samples: Vec<RawSample>,
}

impl RawPhysicalSeries {
    pub fn metric(&self) -> SourceMetric {
        self.metric
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn label(&self, key: &str) -> Option<&str> {
        self.labels.get(key).map(String::as_str)
    }

    pub fn samples(&self) -> &[RawSample] {
        &self.samples
    }
}
#[derive(Clone, Debug)]
pub struct Selection {
    pub cluster: String,
    pub namespace: String,
    pub names: Vec<String>,
    pub metric: SourceMetric,
    pub from: u64,
    pub to: u64,
}
impl Selection {
    pub fn validate(&self) -> crate::Result<()> {
        if !label(&self.cluster)
            || !label(&self.namespace)
            || self.names.is_empty()
            || self.names.len() > 100
            || self.names.iter().any(|v| !label(v))
            || self.from >= self.to
            || self.to - self.from > 86_400
        {
            return Err(crate::AdapterError::Rejected(
                "invalid bounded monitoring selection".into(),
            ));
        }
        Ok(())
    }
}
pub fn label(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 253
        && value.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'-' | b'.' | b'_')
        })
}

#[cfg(test)]
mod tests;
