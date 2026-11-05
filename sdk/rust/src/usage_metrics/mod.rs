//! Authorized measured Sandbox reports. Observed resource-time is not invoice evidence.
mod model;
mod numeric;
use crate::{Client, Error, Result, http::Request, resources::identity};
pub use model::*;
pub use numeric::Quantity;
use reqwest::{Method, StatusCode};
use serde::{Deserialize, Serialize};

/// Optional UTC range, canonical step and metadata selection.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Query {
    /// Inclusive UTC Unix second; default last 24 hours.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<u64>,
    /// Exclusive UTC Unix second, no later than the server's current time.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<u64>,
    /// Allowed multiple of the range's minimum 60/300/3600-second step.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step_seconds: Option<u64>,
    /// Metadata evaluated when the immutable report is generated.
    #[serde(default)]
    pub filters: Filters,
}
/// Metadata selection. Owner routes reject an owner parameter.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Filters {
    /// Administrator-only owner subject filter.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// Current lifecycle statuses.
    #[serde(default)]
    pub statuses: Vec<String>,
    /// Exact immutable runtime version.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime_version: Option<String>,
    /// Exact owned Sandbox.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sandbox_id: Option<String>,
    /// Resolves to an owned Sandbox; measurements remain shared Sandbox usage.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    /// Literal normalized name or ID prefix, maximum 128 characters.
    #[serde(default)]
    pub search: String,
}
/// Server keyset paging and global ordering.
#[derive(Clone, Debug, Default)]
pub struct PageOptions {
    /// Page size from 1 through 200; defaults to 50.
    pub limit: Option<u16>,
    /// Authenticated cursor returned by the same report/scope/sort.
    pub cursor: Option<String>,
    /// Allowlisted field; defaults to last_activity_at.
    pub sort: Option<String>,
    /// `asc` or `desc`, with missing values last in both directions.
    pub order: Option<String>,
}
/// One bounded chart selection.
#[derive(Clone, Debug)]
pub struct SeriesOptions {
    /// `cpu`, `memory` or `disk`.
    pub metric: String,
    /// `total`, `sandbox`, or administrator-only `owner`.
    pub group_by: Option<String>,
    /// Explicit group identity, even when outside the top cohort.
    pub series_id: Option<String>,
    /// Top count from 1 through 20, default 10; the rest form Other.
    pub top: Option<u16>,
}
/// Principal-scoped report methods; every request is reauthorized by the server.
pub struct UsageMetrics<'a> {
    client: &'a Client,
    admin: bool,
}
impl Client {
    /// Access measured usage for the authenticated owner.
    pub fn usage_metrics(&self) -> UsageMetrics<'_> {
        UsageMetrics {
            client: self,
            admin: false,
        }
    }
    /// Access administrator reports; requires ReadAllSandboxes on every request.
    pub fn admin_usage_metrics(&self) -> UsageMetrics<'_> {
        UsageMetrics {
            client: self,
            admin: true,
        }
    }
}
impl UsageMetrics<'_> {
    fn path<'a>(&self, tail: &[&'a str]) -> Vec<&'a str> {
        let mut path = if self.admin {
            vec!["admin", "usage", "metrics", "reports"]
        } else {
            vec!["usage", "metrics", "reports"]
        };
        path.extend_from_slice(tail);
        path
    }
    /// Create/coalesce a cache job, accepting both ready (200) and pending (202).
    pub async fn create(&self, query: &Query) -> Result<Report> {
        if !self.admin && query.filters.owner.is_some() {
            return Err(Error::validation(
                "owner filter requires administrator reporting",
            ));
        }
        let path = self.path(&[]);
        let mut request = Request::operation(Method::POST, &path, Some(query), StatusCode::OK)?;
        request.statuses = vec![StatusCode::OK, StatusCode::ACCEPTED];
        self.client.request(request).await
    }
    /// Read status and summary. No automatic unbounded polling is performed.
    pub async fn report(&self, id: &str) -> Result<Report> {
        let value: Report = self.client.get(&self.path(&[id]), &[]).await?;
        identity(&value.report_id, id)?;
        Ok(value)
    }
    /// Read a globally sorted page from one immutable report.
    pub async fn sandboxes(&self, id: &str, options: &PageOptions) -> Result<Page> {
        if options.limit.is_some_and(|v| !(1..=200).contains(&v)) {
            return Err(Error::validation("limit must be 1 through 200"));
        }
        let mut q = Vec::new();
        if let Some(v) = options.limit {
            q.push(("limit", v.to_string()));
        }
        for (k, v) in [
            ("cursor", &options.cursor),
            ("sort", &options.sort),
            ("order", &options.order),
        ] {
            if let Some(v) = v {
                q.push((k, v.clone()));
            }
        }
        let value: Page = self.client.get(&self.path(&[id, "sandboxes"]), &q).await?;
        identity(&value.report_id, id)?;
        Ok(value)
    }
    /// Search owners across the complete frozen administrator cohort.
    pub async fn owners(
        &self,
        id: &str,
        search: Option<&str>,
        options: &PageOptions,
    ) -> Result<Owners> {
        if !self.admin {
            return Err(Error::validation(
                "owner search requires administrator reporting",
            ));
        }
        let mut q = Vec::new();
        if let Some(v) = search {
            q.push(("search", v.to_owned()));
        }
        if let Some(v) = options.limit {
            q.push(("limit", v.to_string()));
        }
        if let Some(v) = &options.cursor {
            q.push(("cursor", v.clone()));
        }
        let value: Owners = self.client.get(&self.path(&[id, "owners"]), &q).await?;
        identity(&value.report_id, id)?;
        Ok(value)
    }
    /// Read an authorized member of a report.
    pub async fn sandbox(&self, report: &str, id: &str) -> Result<Row> {
        let value: Row = self
            .client
            .get(&self.path(&[report, "sandboxes", id]), &[])
            .await?;
        identity(&value.sandbox_id, id)?;
        Ok(value)
    }
    /// Read full-cohort chart data, independent of table paging.
    pub async fn series(&self, id: &str, options: &SeriesOptions) -> Result<Series> {
        let mut q = vec![("metric", options.metric.clone())];
        for (k, v) in [
            ("group_by", &options.group_by),
            ("series_id", &options.series_id),
        ] {
            if let Some(v) = v {
                q.push((k, v.clone()));
            }
        }
        if let Some(v) = options.top {
            q.push(("top", v.to_string()));
        }
        let value: Series = self.client.get(&self.path(&[id, "series"]), &q).await?;
        identity(&value.report_id, id)?;
        identity(&value.metric, &options.metric)?;
        Ok(value)
    }
}
