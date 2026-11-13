//! Owner document quotas and lifetime Sandbox allocation metering, separate from Agent token usage.
use crate::{Client, Error, PageOptions, ResourcePage, Result, resources::identity};
use serde::Deserialize;

/// Whether the authenticated owner's document quotas are authoritative.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocumentUsageEnforcement {
    /// Document quotas are enforced and their counters are authoritative.
    Enforced,
    /// Document quotas are disabled or their ledger is not ready; counters are unavailable.
    NotEnforced,
}

/// Authoritative usage and limit for one document quota dimension.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct DocumentUsageQuota {
    /// Retained usage charged to the owner.
    pub used: u64,
    /// Maximum owner allowance for this dimension.
    pub limit: u64,
}

/// Document quota enforcement and counters for the authenticated current owner.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct DocumentUsage {
    /// Whether document quota enforcement is active.
    pub enforcement: DocumentUsageEnforcement,
    /// Charged documents and limit, absent when authoritative usage is unavailable.
    pub documents: Option<DocumentUsageQuota>,
    /// Charged retained source bytes and limit, absent when authoritative usage is unavailable.
    pub bytes: Option<DocumentUsageQuota>,
    /// Charged source objects and limit, absent when authoritative usage is unavailable.
    pub objects: Option<DocumentUsageQuota>,
    /// Deleting owner documents; absent while document deletion is disabled.
    pub pending_deletions: Option<u64>,
}

/// Integer allocation-time quantities and display-unit conversions.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct UsageTotals {
    /// Seconds with a bound runtime, including installation and cleanup.
    pub uptime_seconds: u64,
    /// Allocated CPU core-hours.
    pub cpu_core_hours: f64,
    /// Allocated RAM GiB-hours.
    pub memory_gib_hours: f64,
    /// Retained disk GiB-hours, including paused time.
    pub disk_gib_hours: f64,
    /// Exact allocated CPU millicore-seconds.
    pub cpu_millis_seconds: u64,
    /// Exact allocated RAM MiB-seconds.
    pub memory_mib_seconds: u64,
    /// Exact retained disk GiB-seconds.
    pub disk_gib_seconds: u64,
}

/// Allocation during an interval, rather than sampled utilization.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Allocation {
    /// A runtime remains bound.
    pub running: bool,
    /// Allocated CPU millicores.
    pub cpu_millis: u32,
    /// Allocated RAM MiB.
    pub memory_mib: u32,
    /// Retained home capacity in GiB.
    pub disk_gib: u32,
}

/// Retained lifetime usage for one Sandbox.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct SandboxUsage {
    /// Stable Sandbox identity, retained after deletion.
    pub sandbox_id: String,
    /// Owning subject.
    pub owner: String,
    /// Sandbox name.
    pub name: String,
    /// Last recorded lifecycle status.
    pub status: String,
    /// Current allocation for the open interval.
    pub allocation: Allocation,
    /// Creation time in Unix seconds.
    pub created_at: u64,
    /// Earliest metered time in Unix seconds.
    pub tracking_started_at: u64,
    /// Time through which the open interval is projected, in Unix seconds.
    pub as_of: u64,
    /// False when earlier usage or an interval during migration is unknown.
    pub complete_lifecycle: bool,
    /// Lifetime metered quantities.
    pub totals: UsageTotals,
}

/// Owner or administrator lifetime totals.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct UsageSummary {
    /// Projection time in Unix seconds.
    pub as_of: u64,
    /// Measurement semantics: `allocated_resource_time`.
    pub measurement: String,
    /// Number of retained Sandbox usage records.
    pub sandbox_count: u64,
    /// Number with a bound runtime.
    pub running_count: u64,
    /// Records whose pre-metering history is unknown.
    pub incomplete_lifecycle_count: u64,
    /// Earliest metered time, absent for an empty scope.
    pub tracking_started_at: Option<u64>,
    /// Sum over the authorized scope.
    pub totals: UsageTotals,
}

/// Bounded Sandbox usage page, projected at one time.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct UsagePage {
    /// Authorized records in stable order.
    pub items: Vec<SandboxUsage>,
    /// Opaque exclusive continuation cursor.
    pub next: Option<String>,
    /// Projection time in Unix seconds.
    pub as_of: u64,
}

/// Durable closed interval recorded at a lifecycle boundary or checkpoint.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct UsageEvent {
    /// Stable identity for deduplication.
    pub transaction_id: String,
    /// Monotonic usage-ledger revision.
    pub revision: u64,
    /// Associated Sandbox event sequence.
    pub source_seq: u64,
    /// Lifecycle event kind; new kinds are additive.
    pub kind: String,
    /// Inclusive interval start in Unix seconds.
    pub started_at: u64,
    /// Exclusive interval end in Unix seconds.
    pub ended_at: u64,
    /// Lifecycle status following the interval.
    pub status: String,
    /// Allocation during this interval.
    pub allocation: Allocation,
    /// Allocation following the transition.
    pub next_allocation: Allocation,
    /// Quantities accrued during this interval.
    pub quantity: UsageTotals,
}

/// Authorized owner document quota and Sandbox usage operations.
pub struct Usage<'a> {
    client: &'a Client,
}

impl Client {
    /// Access owner document quotas and allocated CPU, RAM, disk and uptime records.
    pub fn usage(&self) -> Usage<'_> {
        Usage { client: self }
    }
}

fn paging(options: &PageOptions) -> Result<Vec<(&'static str, String)>> {
    if options.limit.is_some_and(|value| value == 0 || value > 200)
        || options
            .cursor
            .as_ref()
            .is_some_and(|value| value.is_empty() || value.len() > 256)
    {
        return Err(Error::validation(
            "usage requires a valid cursor and limit from 1 through 200",
        ));
    }
    Ok(options.query())
}

impl Usage<'_> {
    /// Read lifetime totals for the authenticated owner.
    pub async fn summary(&self) -> Result<UsageSummary> {
        self.client.get(&["usage"], &[]).await
    }

    /// Read document quota enforcement and counters for the authenticated owner.
    pub async fn documents(&self) -> Result<DocumentUsage> {
        self.client.get(&["usage", "documents"], &[]).await
    }

    /// List the authenticated owner's retained Sandbox usage records.
    pub async fn sandboxes(&self, options: &PageOptions) -> Result<UsagePage> {
        self.client
            .get(&["usage", "sandboxes"], &paging(options)?)
            .await
    }

    /// Read one authorized Sandbox; administrators may read across owners.
    pub async fn sandbox(&self, id: &str) -> Result<SandboxUsage> {
        let value: SandboxUsage = self.client.get(&["usage", "sandboxes", id], &[]).await?;
        identity(&value.sandbox_id, id)?;
        Ok(value)
    }

    /// Read closed allocation intervals, including retained deleted history.
    pub async fn events(
        &self,
        id: &str,
        options: &PageOptions,
    ) -> Result<ResourcePage<UsageEvent>> {
        self.client
            .get(&["usage", "sandboxes", id, "events"], &paging(options)?)
            .await
    }

    /// Read deployment-wide totals or restrict to one owner; requires administrator authority.
    pub async fn admin_summary(&self, owner: Option<&str>) -> Result<UsageSummary> {
        let query: Vec<_> = owner
            .map(|owner| ("owner", owner.to_owned()))
            .into_iter()
            .collect();
        self.client.get(&["admin", "usage"], &query).await
    }

    /// List all usage records or restrict to one owner; requires administrator authority.
    pub async fn admin_sandboxes(
        &self,
        owner: Option<&str>,
        options: &PageOptions,
    ) -> Result<UsagePage> {
        let mut query = paging(options)?;
        if let Some(owner) = owner {
            query.push(("owner", owner.to_owned()));
        }
        self.client
            .get(&["admin", "usage", "sandboxes"], &query)
            .await
    }
}
