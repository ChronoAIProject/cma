//! Paged allocated resource-hours for an administrator's frozen report cohort.
use super::{Metric, MetricsError, Result};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OwnerAllocationQuery {
    pub metric: Metric,
    pub limit: Option<u16>,
    pub cursor: Option<String>,
}
impl OwnerAllocationQuery {
    pub fn validate(&self) -> Result<u16> {
        let limit = self.limit.unwrap_or(50);
        if !(1..=200).contains(&limit)
            || self
                .cursor
                .as_ref()
                .is_some_and(|v| v.is_empty() || v.len() > 4096)
        {
            return Err(MetricsError::Invalid(
                "Use a valid owner allocation cursor and limit from 1 through 200.".into(),
            ));
        }
        Ok(limit)
    }
    pub fn field(&self) -> &'static str {
        match self.metric {
            Metric::Cpu => "cpu_core_hours",
            Metric::Memory => "memory_gib_hours",
            Metric::Disk => "disk_gib_hours",
        }
    }
    pub fn unit(&self) -> &'static str {
        if self.metric == Metric::Cpu {
            "core-hours"
        } else {
            "GiB-hours"
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OwnerAllocation {
    pub subject: String,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disambiguator: Option<String>,
    pub allocated_hours: f64,
    pub sandbox_count: u64,
    pub complete: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OwnerAllocations {
    pub report_id: String,
    pub metric: Metric,
    pub unit: String,
    pub items: Vec<OwnerAllocation>,
    pub next: Option<String>,
    pub total_count: u64,
}
