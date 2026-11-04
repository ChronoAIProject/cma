use super::{Metric, Range, ReportQuery};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scope {
    pub principal: String,
    pub admin: bool,
}

/// Independent compute/home intervals. Names only select source data; retained
/// UID/incarnation proof and these time bounds authorize attribution.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub id: String,
    pub schema_version: u32,
    pub revision: u64,
    pub deployment: String,
    pub cluster: String,
    pub namespace: String,
    pub native_sandbox_uid: String,
    pub claim_uid: String,
    pub sandbox_id: String,
    pub owner_subject: String,
    pub pod_name: Option<String>,
    pub pod_uid: Option<String>,
    pub runtime_container_id: Option<String>,
    pub restart_count: Option<i32>,
    pub runtime_generation: Option<u64>,
    pub pvc_name: String,
    pub pvc_uid: String,
    pub kind: BindingKind,
    pub cpu_limit_cores: Option<f64>,
    pub memory_limit_bytes: Option<f64>,
    pub valid_from: u64,
    pub valid_until: Option<u64>,
    pub observed_at: u64,
    pub proof_version: u32,
    pub proof_status: ProofStatus,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BindingKind {
    Compute,
    Home,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProofStatus {
    Verified,
    Unattributed,
}
impl Binding {
    pub fn range(&self, cutoff: u64) -> Range {
        // Observation failures cannot keep an open assignment alive forever.
        Range {
            from: self.valid_from,
            to: self
                .valid_until
                .unwrap_or(self.observed_at)
                .min(cutoff)
                .max(self.valid_from),
        }
    }
    pub fn physical_key(&self) -> String {
        format!(
            "{}:{}:{}:{}",
            self.cluster,
            self.namespace,
            if self.kind == BindingKind::Compute {
                "pod"
            } else {
                "pvc"
            },
            if self.kind == BindingKind::Compute {
                self.pod_uid.as_deref().unwrap_or("")
            } else {
                &self.pvc_uid
            }
        )
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Aggregate {
    pub integral: f64,
    pub display_integral: f64,
    pub capacity_usage_integral: f64,
    pub covered_seconds: f64,
    pub expected_seconds: f64,
    pub capacity_integral: f64,
    pub capacity_covered_seconds: f64,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub observed_at: Option<f64>,
    pub latest: Option<f64>,
    pub latest_capacity: Option<f64>,
    pub available_from: Option<f64>,
}
impl Aggregate {
    pub fn mean(&self) -> Option<f64> {
        (self.covered_seconds > 0.0).then(|| self.display_integral / self.covered_seconds)
    }
    pub fn percent(&self) -> Option<f64> {
        (self.capacity_integral > 0.0)
            .then(|| 100.0 * self.capacity_usage_integral / self.capacity_integral)
    }
    pub fn coverage(&self) -> Option<f64> {
        (self.expected_seconds > 0.0)
            .then(|| (self.covered_seconds / self.expected_seconds).min(1.0))
    }
    /// Add resource-time statistics, never average percentages.
    pub fn merge(&mut self, other: &Self) {
        self.integral += other.integral;
        self.display_integral += other.display_integral;
        self.capacity_usage_integral += other.capacity_usage_integral;
        self.covered_seconds += other.covered_seconds;
        self.expected_seconds += other.expected_seconds;
        self.capacity_integral += other.capacity_integral;
        self.capacity_covered_seconds += other.capacity_covered_seconds;
        self.min = option_min(self.min, other.min);
        self.max = option_max(self.max, other.max);
        self.available_from = option_min(self.available_from, other.available_from);
        if other.observed_at > self.observed_at {
            self.observed_at = other.observed_at;
            self.latest = other.latest;
            self.latest_capacity = other.latest_capacity;
        }
    }
}
pub fn option_min(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}
pub fn option_max(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Minute {
    pub from: u64,
    pub to: u64,
    pub metrics: [Aggregate; 3],
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetricRollup {
    pub schema_version: u32,
    pub algorithm_version: u32,
    pub source: String,
    pub sandbox_id: String,
    pub owner_subject: String,
    pub day: u64,
    pub chunk: u16,
    pub mapping_revision: u64,
    pub cutoff: u64,
    pub source_complete: bool,
    pub minutes: Vec<Minute>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AllocatedPeriod {
    pub disk_retained_seconds: f64,
    pub uptime_seconds: f64,
    pub cpu_core_hours: f64,
    pub memory_gib_hours: f64,
    pub disk_gib_hours: f64,
    pub complete: bool,
    pub projected_tail_seconds: u64,
    pub unknown_seconds: u64,
}
impl AllocatedPeriod {
    pub fn merge(&mut self, value: &Self) {
        self.disk_retained_seconds += value.disk_retained_seconds;
        self.uptime_seconds += value.uptime_seconds;
        self.cpu_core_hours += value.cpu_core_hours;
        self.memory_gib_hours += value.memory_gib_hours;
        self.disk_gib_hours += value.disk_gib_hours;
        self.complete &= value.complete;
        self.projected_tail_seconds += value.projected_tail_seconds;
        self.unknown_seconds += value.unknown_seconds;
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capacity {
    pub cpu_cores: f64,
    pub memory_bytes: f64,
    pub disk_bytes: f64,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Owner {
    pub subject: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricState {
    Ok,
    Partial,
    Stale,
    Unmounted,
    NotRunning,
    Unavailable,
    Unattributed,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Measurement {
    pub state: MetricState,
    pub observed_at: Option<f64>,
    pub source: String,
    pub covered_seconds: f64,
    pub expected_seconds: f64,
    pub unknown_mapping_seconds: f64,
    pub coverage_ratio: Option<f64>,
    pub available_from: Option<f64>,
    pub latest: Option<f64>,
    pub last_observed: Option<f64>,
    pub mean: Option<f64>,
    pub max: Option<f64>,
    pub observed_hours: Option<f64>,
    pub percent_of_limit: Option<f64>,
    pub filesystem_capacity_bytes: Option<f64>,
    pub exceeds_capacity: bool,
}
impl Measurement {
    pub fn project(
        value: &Aggregate,
        metric: Metric,
        now: u64,
        running: bool,
        unknown: f64,
    ) -> Self {
        let fresh = value
            .observed_at
            .is_some_and(|at| now as f64 - at <= super::FRESHNESS_SECONDS as f64);
        let percent = value.percent();
        let state = if !running && metric != Metric::Disk {
            MetricState::NotRunning
        } else if !running {
            MetricState::Unmounted
        } else if value.observed_at.is_none() {
            MetricState::Unavailable
        } else if !fresh {
            MetricState::Stale
        } else if unknown > 0.0 || value.coverage().is_none_or(|v| v < 0.999) {
            MetricState::Partial
        } else {
            MetricState::Ok
        };
        Self {
            state,
            observed_at: value.observed_at,
            source: super::SOURCE.into(),
            covered_seconds: value.covered_seconds,
            expected_seconds: value.expected_seconds,
            unknown_mapping_seconds: unknown,
            coverage_ratio: value.coverage(),
            available_from: value.available_from,
            latest: if fresh && running { value.latest } else { None },
            last_observed: value.latest,
            mean: value.mean(),
            max: value.max,
            observed_hours: (value.covered_seconds > 0.0).then(|| {
                value.integral
                    / 3600.0
                    / if metric == Metric::Cpu {
                        1.0
                    } else {
                        1_073_741_824.0
                    }
            }),
            percent_of_limit: percent,
            exceeds_capacity: percent.is_some_and(|v| v > 100.0),
            filesystem_capacity_bytes: (metric == Metric::Disk)
                .then_some(value.latest_capacity)
                .flatten(),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Measured {
    pub cpu: Measurement,
    pub memory: Measurement,
    pub disk: Measurement,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Billing {
    pub basis: String,
    pub status: String,
    pub charges: Option<f64>,
}
impl Default for Billing {
    fn default() -> Self {
        Self {
            basis: "allocated".into(),
            status: "not_integrated".into(),
            charges: None,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Row {
    pub sandbox_id: String,
    pub name: String,
    pub owner: Owner,
    pub status: String,
    pub runtime_version: Option<String>,
    pub agent_count: u64,
    pub last_activity_at: Option<u64>,
    pub allocation: Capacity,
    pub allocated_period: AllocatedPeriod,
    pub measured: Measured,
    pub billing: Billing,
    pub shared_sandbox_usage: bool,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Summary {
    pub sandbox_count: u64,
    pub running_count: u64,
    pub allocated_period: AllocatedPeriod,
    pub total_allocated_capacity: Capacity,
    pub covered_allocated_capacity: Capacity,
    pub measured: Option<Measured>,
    pub excluded_count: u64,
    pub infrastructure: Option<Infrastructure>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Infrastructure {
    pub unassigned_pool: [Option<f64>; 3],
    pub unattributed: [Option<f64>; 3],
    pub observed_at: Option<u64>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportStatus {
    Pending,
    Ready,
    Failed,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Report {
    pub schema_version: u32,
    pub report_id: String,
    pub status: ReportStatus,
    pub report_complete: bool,
    pub scope: Scope,
    pub query: ReportQuery,
    pub as_of: u64,
    pub measurement_cutoff: u64,
    pub effective_range: Range,
    pub expires_at: u64,
    pub allocation_watermark: String,
    pub mapping_revision: u64,
    pub available_from: Option<u64>,
    pub tracking_started_at: Option<u64>,
    pub warnings: Vec<String>,
    pub source_health: String,
    pub summary: Option<Summary>,
    pub poll_url: String,
    pub retry_after_seconds: u16,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rows {
    pub items: Vec<Row>,
    pub next: Option<String>,
    pub report_id: String,
    pub as_of: u64,
    pub total_count: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Point {
    pub from: u64,
    pub to: u64,
    pub mean: Option<f64>,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub covered_seconds: f64,
    pub expected_seconds: f64,
    pub unknown_mapping_seconds: f64,
    pub numerator: Option<f64>,
    pub denominator: Option<f64>,
    pub reason: Option<String>,
    pub excluded_count: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Line {
    pub id: String,
    pub label: String,
    pub points: Vec<Point>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Series {
    pub report_id: String,
    pub metric: Metric,
    pub unit: String,
    pub source: String,
    pub lines: Vec<Line>,
}
