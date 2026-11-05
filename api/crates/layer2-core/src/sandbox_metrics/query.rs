use serde::{Deserialize, Serialize};

use super::{MetricsError, Result, FINALIZATION_DELAY_SECONDS, RETENTION_SECONDS};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Range {
    pub from: u64,
    pub to: u64,
}
impl Range {
    pub fn seconds(self) -> u64 {
        self.to.saturating_sub(self.from)
    }
    pub fn intersect(self, other: Self) -> Self {
        let from = self.from.max(other.from);
        Self {
            from,
            to: self.to.min(other.to).max(from),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Filters {
    pub owner: Option<String>,
    pub statuses: Vec<Status>,
    pub runtime_version: Option<String>,
    pub sandbox_id: Option<String>,
    pub agent_id: Option<String>,
    pub search: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Provisioning,
    Installing,
    Active,
    Pausing,
    Paused,
    Reviving,
    CredentialRotating,
    Rotating,
    Retiring,
    Retired,
    Failed,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ReportInput {
    pub from: Option<u64>,
    pub to: Option<u64>,
    pub step_seconds: Option<u64>,
    pub filters: Filters,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReportQuery {
    pub from: u64,
    pub to: u64,
    pub step_seconds: u64,
    pub filters: Filters,
}
impl ReportQuery {
    pub fn range(&self) -> Range {
        Range {
            from: self.from,
            to: self.to,
        }
    }
    pub fn effective_range(&self, now: u64) -> Range {
        Range {
            from: self.from,
            to: self
                .to
                .min(now.saturating_sub(FINALIZATION_DELAY_SECONDS))
                .max(self.from),
        }
    }
}
impl ReportInput {
    pub fn validate(mut self, now: u64) -> Result<ReportQuery> {
        let to = self.to.unwrap_or(now / 60 * 60);
        let from = self.from.unwrap_or(to.saturating_sub(86_400));
        if from >= to || to > now || to - from > RETENTION_SECONDS {
            return Err(invalid(
                "Use a past half-open UTC range of at most 30 days.",
            ));
        }
        let minimum = match to - from {
            0..=86_400 => 60,
            86_401..=604_800 => 300,
            _ => 3600,
        };
        // Day-dividing multiples preserve canonical buckets across daily read
        // model partitions. Unaligned seven-day windows need a coarser default
        // because both partial boundary buckets count toward the point budget.
        let allowed = [
            60, 120, 180, 240, 300, 600, 900, 1200, 1800, 3600, 7200, 10800, 14400, 21600, 43200,
            86400,
        ];
        let canonical = minimum;
        let minimum = allowed
            .into_iter()
            .find(|step| {
                *step >= minimum && *step % minimum == 0 && (to - 1) / step - from / step < 2016
            })
            .ok_or_else(|| invalid("No supported chart resolution."))?;
        let step = self.step_seconds.unwrap_or(minimum);
        if step < minimum || !allowed.contains(&step) || !step.is_multiple_of(canonical) {
            return Err(invalid(&format!(
                "step_seconds must be an allowed day-dividing multiple; minimum {minimum} seconds."
            )));
        }
        for (id, prefix) in [
            (&self.filters.sandbox_id, "sbx_"),
            (&self.filters.agent_id, "agt_"),
        ] {
            if id.as_deref().is_some_and(|s| !opaque_id(s, prefix)) {
                return Err(invalid("Invalid resource ID."));
            }
        }
        if self
            .filters
            .owner
            .as_deref()
            .is_some_and(|v| !literal(v, 256))
            || self
                .filters
                .runtime_version
                .as_deref()
                .is_some_and(|v| !literal(v, 512))
        {
            return Err(invalid("Invalid owner or runtime version."));
        }
        self.filters.search = self.filters.search.trim().to_lowercase();
        if !self.filters.search.is_empty() && !literal(&self.filters.search, 128) {
            return Err(invalid(
                "Search must be a literal name or ID prefix of at most 128 characters.",
            ));
        }
        self.filters.statuses.sort();
        self.filters.statuses.dedup();
        Ok(ReportQuery {
            from,
            to,
            step_seconds: step,
            filters: self.filters,
        })
    }
}
pub fn opaque_id(value: &str, prefix: &str) -> bool {
    value.strip_prefix(prefix).is_some_and(|tail| {
        tail.len() == 32
            && tail
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    })
}
fn literal(value: &str, max: usize) -> bool {
    !value.trim().is_empty() && value.len() <= max && !value.chars().any(char::is_control)
}
fn invalid(message: &str) -> MetricsError {
    MetricsError::Invalid(message.into())
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Sort {
    Name,
    Owner,
    Status,
    #[default]
    LastActivityAt,
    CpuMeanCores,
    MemoryMeanBytes,
    DiskLatestBytes,
    AllocatedCpuCoreHours,
    AllocatedMemoryGibHours,
    AllocatedDiskGibHours,
}
impl Sort {
    pub const ALL: [Self; 10] = [
        Self::Name,
        Self::Owner,
        Self::Status,
        Self::LastActivityAt,
        Self::CpuMeanCores,
        Self::MemoryMeanBytes,
        Self::DiskLatestBytes,
        Self::AllocatedCpuCoreHours,
        Self::AllocatedMemoryGibHours,
        Self::AllocatedDiskGibHours,
    ];
    pub fn key(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::Owner => "owner",
            Self::Status => "status",
            Self::LastActivityAt => "last_activity_at",
            Self::CpuMeanCores => "cpu_mean_cores",
            Self::MemoryMeanBytes => "memory_mean_bytes",
            Self::DiskLatestBytes => "disk_latest_bytes",
            Self::AllocatedCpuCoreHours => "allocated_cpu_core_hours",
            Self::AllocatedMemoryGibHours => "allocated_memory_gib_hours",
            Self::AllocatedDiskGibHours => "allocated_disk_gib_hours",
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Order {
    Asc,
    #[default]
    Desc,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PageQuery {
    pub limit: Option<u16>,
    pub cursor: Option<String>,
    pub sort: Sort,
    pub order: Order,
}
impl PageQuery {
    pub fn validate(&self, admin: bool) -> Result<u16> {
        let limit = self.limit.unwrap_or(50);
        if !(1..=200).contains(&limit) || self.cursor.as_ref().is_some_and(|v| v.len() > 4096) {
            return Err(invalid("Use a valid cursor and limit from 1 through 200."));
        }
        if self.sort == Sort::Owner && !admin {
            return Err(MetricsError::Forbidden);
        }
        Ok(limit)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Metric {
    Cpu,
    Memory,
    Disk,
}
impl Metric {
    pub fn index(self) -> usize {
        match self {
            Self::Cpu => 0,
            Self::Memory => 1,
            Self::Disk => 2,
        }
    }
    pub fn unit(self) -> &'static str {
        if self == Self::Cpu {
            "cores"
        } else {
            "bytes"
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroupBy {
    #[default]
    Total,
    Owner,
    Sandbox,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SeriesQuery {
    pub metric: Metric,
    #[serde(default)]
    pub group_by: GroupBy,
    pub series_id: Option<String>,
    pub top: Option<u16>,
}
impl SeriesQuery {
    pub fn validate(&self, admin: bool) -> Result<usize> {
        if self.group_by == GroupBy::Owner && !admin {
            return Err(MetricsError::Forbidden);
        }
        let top = self.top.unwrap_or(10);
        if !(1..=20).contains(&top)
            || self.series_id.as_deref().is_some_and(|s| !literal(s, 256))
            || (self.group_by == GroupBy::Total && self.series_id.is_some())
            || (self.group_by == GroupBy::Sandbox
                && self
                    .series_id
                    .as_deref()
                    .is_some_and(|id| !opaque_id(id, "sbx_")))
        {
            return Err(invalid(
                "Invalid series selection; top must be from 1 through 20.",
            ));
        }
        Ok(usize::from(top))
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct OwnerQuery {
    pub search: String,
    pub limit: Option<u16>,
    pub cursor: Option<String>,
}
impl OwnerQuery {
    pub fn validate(&self) -> Result<u16> {
        if !self.search.is_empty() && !literal(&self.search, 128) {
            return Err(invalid(
                "Owner search must be a literal prefix of at most 128 characters.",
            ));
        }
        PageQuery {
            limit: self.limit,
            cursor: self.cursor.clone(),
            ..Default::default()
        }
        .validate(true)
    }
}
