//! Paged allocation totals for a frozen administrator report.
use super::{Quantity, UsageMetrics, numeric};
use crate::{Error, Result, resources::identity};
use serde::{Deserialize, Serialize};

/// Allocation resource and bounded continuation.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OwnerAllocationQuery {
    /// `cpu`, `memory` or `disk`.
    pub metric: String,
    /// Page size from 1 through 200; default 50.
    pub limit: Option<u16>,
    /// Signed continuation from this report, resource and administrator.
    pub cursor: Option<String>,
}
/// One owner, ordered by allocation descending then opaque identity.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct OwnerAllocation {
    /// Opaque identity for keys only; never use as display text.
    pub subject: String,
    /// Display name or safe unnamed fallback, matching filter options.
    pub label: String,
    /// Secondary context only for actual duplicate names.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disambiguator: Option<String>,
    /// Recorded allocation in the response's unit.
    pub allocated_hours: Quantity,
    /// Number of report Sandboxes attributed to this owner.
    #[serde(deserialize_with = "numeric::integer")]
    pub sandbox_count: u64,
    /// Whether all contributing allocation histories are complete.
    pub complete: bool,
}
/// Bounded page across all owners in the immutable report.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct OwnerAllocations {
    /// Frozen report identity.
    pub report_id: String,
    /// Selected resource.
    pub metric: String,
    /// `core-hours` or `GiB-hours`.
    pub unit: String,
    /// At most the requested number of rows.
    pub items: Vec<OwnerAllocation>,
    /// Signed continuation; absent at the end of the cohort.
    pub next: Option<String>,
    /// Whole report owner count, not just this page.
    #[serde(deserialize_with = "numeric::integer")]
    pub total_count: u64,
}
impl UsageMetrics<'_> {
    /// Read ranked allocations without creating or changing a report.
    pub async fn owner_allocations(
        &self,
        id: &str,
        options: &OwnerAllocationQuery,
    ) -> Result<OwnerAllocations> {
        if !self.admin {
            return Err(Error::validation(
                "owner allocation requires administrator reporting",
            ));
        }
        if options.limit.is_some_and(|v| !(1..=200).contains(&v)) {
            return Err(Error::validation(
                "owner allocation limit must be from 1 through 200",
            ));
        }
        let mut query = vec![("metric", options.metric.clone())];
        if let Some(limit) = options.limit {
            query.push(("limit", limit.to_string()));
        }
        if let Some(cursor) = &options.cursor {
            query.push(("cursor", cursor.clone()));
        }
        let value: OwnerAllocations = self
            .client
            .get(&self.path(&[id, "owner-allocations"]), &query)
            .await?;
        identity(&value.report_id, id)?;
        identity(&value.metric, &options.metric)?;
        identity(
            &value.unit,
            if options.metric == "cpu" {
                "core-hours"
            } else {
                "GiB-hours"
            },
        )?;
        Ok(value)
    }
}
