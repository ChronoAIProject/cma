use super::{Quantity, numeric};
use serde::Deserialize;

/// Half-open UTC Unix-second interval.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Range {
    /// Inclusive start.
    #[serde(deserialize_with = "numeric::integer")]
    pub from: u64,
    /// Exclusive end.
    #[serde(deserialize_with = "numeric::integer")]
    pub to: u64,
}
/// Canonical, frozen report query.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct CanonicalQuery {
    /// Requested start.
    #[serde(deserialize_with = "numeric::integer")]
    pub from: u64,
    /// Requested end, which can exceed the finalized cutoff.
    #[serde(deserialize_with = "numeric::integer")]
    pub to: u64,
    /// Canonical chart step.
    #[serde(deserialize_with = "numeric::integer")]
    pub step_seconds: u64,
    /// Metadata selection.
    pub filters: super::Filters,
}
/// Cache authority; an ID alone grants no access.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Scope {
    /// Authenticated principal.
    pub principal: String,
    /// Whether this is an administrator route scope.
    pub admin: bool,
}
/// Ready or pending immutable reporting snapshot.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Report {
    /// Version of this public contract.
    pub schema_version: u32,
    /// Opaque report identifier.
    pub report_id: String,
    /// `pending`, `ready`, or `failed`.
    pub status: String,
    /// Whole-cohort computation completeness, independent of metric coverage.
    pub report_complete: bool,
    /// Principal/admin cache scope.
    pub scope: Scope,
    /// Canonical request.
    pub query: CanonicalQuery,
    /// Snapshot generation boundary.
    #[serde(deserialize_with = "numeric::integer")]
    pub as_of: u64,
    /// Finalized data cutoff, normally server time minus 120 seconds.
    #[serde(deserialize_with = "numeric::integer")]
    pub measurement_cutoff: u64,
    /// Actual interval included in period aggregates.
    pub effective_range: Range,
    /// Logical cache expiry; refresh after HTTP 410.
    #[serde(deserialize_with = "numeric::integer")]
    pub expires_at: u64,
    /// Frozen allocation source watermark.
    pub allocation_watermark: String,
    /// Retained identity revision.
    #[serde(deserialize_with = "numeric::integer")]
    pub mapping_revision: u64,
    /// Earliest available measurement.
    #[serde(default, deserialize_with = "numeric::optional_integer")]
    pub available_from: Option<u64>,
    /// First proven tracking time.
    #[serde(default, deserialize_with = "numeric::optional_integer")]
    pub tracking_started_at: Option<u64>,
    /// Explicit coverage/source warnings.
    pub warnings: Vec<String>,
    /// Source health independent of lifecycle state.
    pub source_health: String,
    /// Whole selected cohort, available when ready.
    pub summary: Option<Summary>,
    /// Authorized polling route.
    pub poll_url: String,
    /// Suggested bounded polling interval.
    pub retry_after_seconds: u16,
}
/// Capacity is kept distinct from measured usage.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Capacity {
    /// Allocated CPU cores.
    pub cpu_cores: Quantity,
    /// Allocated memory bytes.
    pub memory_bytes: Quantity,
    /// Provisioned home bytes.
    pub disk_bytes: Quantity,
}
/// Clipped allocated resource-time, derived without modifying the billing ledger.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct AllocatedPeriod {
    /// Seconds retaining a home.
    pub disk_retained_seconds: Quantity,
    /// Seconds with a bound runtime.
    pub uptime_seconds: Quantity,
    /// Allocated CPU core-hours.
    pub cpu_core_hours: Quantity,
    /// Allocated memory GiB-hours.
    pub memory_gib_hours: Quantity,
    /// Allocated retained storage GiB-hours.
    pub disk_gib_hours: Quantity,
    /// Allocation history completeness, independent of metric coverage.
    pub complete: bool,
    /// Current allocation projected to the cutoff.
    #[serde(deserialize_with = "numeric::integer")]
    pub projected_tail_seconds: u64,
    /// Missing allocation evidence duration.
    #[serde(deserialize_with = "numeric::integer")]
    pub unknown_seconds: u64,
}
/// Coverage, freshness and source metadata shared by each measured resource.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Measurement {
    /// `ok`, `partial`, `stale`, `unmounted`, `not_running`, `unavailable`, or `unattributed`.
    pub state: String,
    /// Actual source timestamp, never a query evaluation timestamp.
    pub observed_at: Option<Quantity>,
    /// Versioned source cohort.
    pub source: String,
    /// Supported measured duration.
    pub covered_seconds: Quantity,
    /// Expected runtime/home duration.
    pub expected_seconds: Quantity,
    /// Duration whose physical ownership could not be proved.
    pub unknown_mapping_seconds: Quantity,
    /// Fraction of expected duration with measurements.
    pub coverage_ratio: Option<Quantity>,
    /// Earliest supported sample interval.
    pub available_from: Option<Quantity>,
    /// Last observed value, retained even when stale.
    pub last_observed: Option<Quantity>,
    /// Ratio using the same covered usage/capacity subset.
    pub percent_of_limit: Option<Quantity>,
    /// PVC filesystem capacity, distinct from provisioned bytes.
    pub filesystem_capacity_bytes: Option<Quantity>,
    /// Diagnostic for values above 100%; values are not clamped.
    pub exceeds_capacity: bool,
}
/// CPU counter-derived measurements.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Cpu {
    /// Shared metric metadata.
    #[serde(flatten)]
    pub coverage: Measurement,
    /// Fresh two-minute counter rate in cores.
    pub latest_cores: Option<Quantity>,
    /// Time-weighted period mean.
    pub mean_cores: Option<Quantity>,
    /// Observed rate envelope maximum.
    pub max_cores: Option<Quantity>,
    /// Supported counter deltas divided by 3600; observational estimate.
    pub observed_core_hours: Option<Quantity>,
}
/// RAM working-set or PVC filesystem-used measurement.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Gauge {
    /// Shared metric metadata.
    #[serde(flatten)]
    pub coverage: Measurement,
    /// Fresh bytes or null.
    pub latest_bytes: Option<Quantity>,
    /// Time-weighted period mean bytes.
    pub mean_bytes: Option<Quantity>,
    /// Observed byte envelope maximum.
    pub max_bytes: Option<Quantity>,
    /// Bounded gauge integral in GiB-hours; observational estimate.
    pub observed_gib_hours: Option<Quantity>,
}
/// The three measured resources.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Measured {
    /// CPU consumption.
    pub cpu: Cpu,
    /// Memory working set, not RSS or allocated memory.
    pub memory: Gauge,
    /// PVC filesystem used, counting each physical PVC once.
    pub disk: Gauge,
}
/// Explicit separation of reporting from billing integration.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Billing {
    /// Existing allocated basis.
    pub basis: String,
    /// `not_integrated` in this reporting contract.
    pub status: String,
    /// Null until a separately approved billing contract is integrated.
    pub charges: Option<Quantity>,
}
/// Owner identity, supplied by CMA authority rather than monitoring labels.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Owner {
    /// Stable subject ID.
    pub subject: String,
    /// Optional administrator-only display name.
    pub display_name: Option<String>,
}
/// One frozen Sandbox projection, shared by all of its Agents.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Row {
    /// Stable Sandbox ID.
    pub sandbox_id: String,
    /// Frozen name.
    pub name: String,
    /// Authoritative owner.
    pub owner: Owner,
    /// Lifecycle status at snapshot generation.
    pub status: String,
    /// Immutable runtime version when known.
    pub runtime_version: Option<String>,
    /// Number of non-retired Agents sharing this Sandbox.
    #[serde(deserialize_with = "numeric::integer")]
    pub agent_count: u64,
    /// Last activity timestamp when available.
    #[serde(default, deserialize_with = "numeric::optional_integer")]
    pub last_activity_at: Option<u64>,
    /// Reserved capacity.
    pub allocation: Capacity,
    /// Clipped allocated resource-time.
    pub allocated_period: AllocatedPeriod,
    /// Measured consumption and independent coverage.
    pub measured: Measured,
    /// Billing integration declaration.
    pub billing: Billing,
    /// True for Agent-selected reports; consumption is shared, not exclusive.
    pub shared_sandbox_usage: bool,
}
/// Administrator-only unattributed infrastructure diagnostics.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Infrastructure {
    /// `ok`, `partial`, `source_unavailable` or `stale`.
    pub state: String,
    /// Physical observations or metrics whose proof/measurement is incomplete.
    #[serde(deserialize_with = "numeric::integer")]
    pub unproven_count: u64,
    /// Unassigned warm pool CPU/RAM/PVC values; null when unknown.
    pub unassigned_pool: [Option<Quantity>; 3],
    /// Unresolved assignment CPU/RAM/PVC values; never owner-billable totals.
    pub unattributed: [Option<Quantity>; 3],
    /// Diagnostic source time.
    #[serde(default, deserialize_with = "numeric::optional_integer")]
    pub observed_at: Option<u64>,
}
/// Complete selected cohort independent of any visible table page.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Summary {
    /// Total Sandbox count.
    #[serde(deserialize_with = "numeric::integer")]
    pub sandbox_count: u64,
    /// Count with bound runtimes.
    #[serde(deserialize_with = "numeric::integer")]
    pub running_count: u64,
    /// Allocated period totals.
    pub allocated_period: AllocatedPeriod,
    /// All selected reserved capacity.
    pub total_allocated_capacity: Capacity,
    /// Reserved capacity in the covered measurement subset.
    pub covered_allocated_capacity: Capacity,
    /// Aggregate measurements and ratios of covered sums.
    pub measured: Option<Measured>,
    /// Sandboxes excluded by incomplete measurement coverage.
    #[serde(deserialize_with = "numeric::integer")]
    pub excluded_count: u64,
    /// Administrator-only diagnostic footer.
    pub infrastructure: Option<Infrastructure>,
}
/// Stable keyset page.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Page {
    /// Frozen authorized rows.
    pub items: Vec<Row>,
    /// Opaque authenticated continuation, or null.
    pub next: Option<String>,
    /// Snapshot identity.
    pub report_id: String,
    /// Snapshot time.
    #[serde(deserialize_with = "numeric::integer")]
    pub as_of: u64,
    /// Complete cohort size.
    #[serde(deserialize_with = "numeric::integer")]
    pub total_count: u64,
}
/// One canonical chart bucket.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Point {
    /// Inclusive bucket start.
    #[serde(deserialize_with = "numeric::integer")]
    pub from: u64,
    /// Exclusive bucket end.
    #[serde(deserialize_with = "numeric::integer")]
    pub to: u64,
    /// Weighted mean or null for missing measurements.
    pub mean: Option<Quantity>,
    /// Observed envelope minimum.
    pub min: Option<Quantity>,
    /// Observed envelope maximum.
    pub max: Option<Quantity>,
    /// Covered resource-seconds.
    pub covered_seconds: Quantity,
    /// Expected resource-seconds.
    pub expected_seconds: Quantity,
    /// Duration without proven identity or allocation evidence.
    pub unknown_mapping_seconds: Quantity,
    /// Integrated usage on the covered subset.
    pub numerator: Option<Quantity>,
    /// Integrated capacity on the same covered subset.
    pub denominator: Option<Quantity>,
    /// Explicit missing/partial reason.
    pub reason: Option<String>,
    /// Excluded or partially covered Sandboxes.
    #[serde(deserialize_with = "numeric::integer")]
    pub excluded_count: u64,
}
/// Named total, owner, Sandbox or Other line.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Line {
    /// Group identity.
    pub id: String,
    /// Presentation label.
    pub label: String,
    /// Canonical buckets with explicit gaps.
    pub points: Vec<Point>,
}
/// Full selected cohort chart result.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Series {
    /// Frozen report identity.
    pub report_id: String,
    /// Selected `cpu`, `memory` or `disk` resource.
    pub metric: String,
    /// `cores` or `bytes`.
    pub unit: String,
    /// Versioned source cohort.
    pub source: String,
    /// Top groups plus Other, or an explicit selected group.
    pub lines: Vec<Line>,
}

/// Administrator owner search over the complete report cohort.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Owners {
    /// Stable owner subjects, optionally with display names.
    pub items: Vec<Owner>,
    /// Authenticated prefix-bound continuation.
    pub next: Option<String>,
    /// Immutable report identity.
    pub report_id: String,
}
