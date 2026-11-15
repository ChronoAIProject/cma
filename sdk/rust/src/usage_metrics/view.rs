//! Retained snapshots expose age independently of a running replacement.
use super::*;
/// Rolling window or explicit historical range.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ViewWindow {
    /// Last 24 hours.
    #[default]
    #[serde(rename = "24h")]
    Day,
    /// Last seven days.
    #[serde(rename = "7d")]
    Week,
    /// Last 30 days.
    #[serde(rename = "30d")]
    Month,
    /// Explicit from/to range.
    #[serde(rename = "custom")]
    Custom,
}
impl ViewWindow {
    fn name(self) -> &'static str {
        match self {
            Self::Day => "24h",
            Self::Week => "7d",
            Self::Month => "30d",
            Self::Custom => "custom",
        }
    }
}
/// Stable selection for the newest committed report.
#[derive(Clone, Debug, Default)]
pub struct ViewQuery {
    /// Rolling or custom range, default last 24 hours.
    pub window: ViewWindow,
    /// Custom range and filters; rolling windows omit from/to.
    pub query: Query,
    /// Request a replacement even while fresh.
    pub refresh: bool,
}
/// Replacement scheduling disposition.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefreshState {
    /// No replacement needed.
    Idle,
    /// A replacement is admitted.
    Pending,
    /// Capacity or dependency deferred a replacement.
    Deferred,
}
/// Refresh observation, never an authorization grant.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ViewRefresh {
    /// Scheduling disposition.
    pub state: RefreshState,
    /// Admitted report identity.
    pub report_id: Option<String>,
    /// Minimum observation retry delay.
    #[serde(deserialize_with = "numeric::integer")]
    pub retry_after_seconds: u64,
}
/// Immutable snapshot plus current requested range and age.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ReportView {
    /// Contract version.
    pub schema_version: u32,
    /// Requested window class.
    pub window: ViewWindow,
    /// Current requested range; snapshot has its own effective range.
    pub requested_range: Range,
    /// Ready snapshot or actually admitted pending report.
    pub report: Report,
    /// Completion time; absent for pending.
    #[serde(deserialize_with = "numeric::optional_integer")]
    pub generated_at: Option<u64>,
    /// Snapshot exceeds the server freshness threshold.
    pub stale: bool,
    /// Independent replacement state.
    pub refresh: ViewRefresh,
}
/// Only explicit feature/route absence permits old-endpoint fallback.
pub fn view_unavailable(error: &Error) -> bool {
    matches!(error, Error::Api { status: 501, code: Some(code), .. } if code == "report_view_disabled")
        || matches!(
            error,
            Error::Api {
                status: 404,
                code: None,
                extensions,
                ..
            } if extensions.is_empty()
        )
}
impl UsageMetrics<'_> {
    /// Return a committed summary without waiting for its replacement.
    pub async fn latest(&self, input: &ViewQuery) -> Result<ReportView> {
        let query = &input.query;
        if (!self.admin && query.filters.owner.is_some())
            || (input.window == ViewWindow::Custom && (query.from.is_none() || query.to.is_none()))
            || (input.window != ViewWindow::Custom && (query.from.is_some() || query.to.is_some()))
        {
            return Err(Error::validation(
                "invalid usage view range or owner selection",
            ));
        }
        let path = if self.admin {
            vec!["admin", "usage", "metrics", "view"]
        } else {
            vec!["usage", "metrics", "view"]
        };
        let mut values = vec![("window", input.window.name().to_owned())];
        if input.refresh {
            values.push(("refresh", "true".into()));
        }
        if query.filters != Filters::default() {
            values.push((
                "filters",
                serde_json::to_string(&query.filters)
                    .map_err(|_| Error::validation("invalid usage filters"))?,
            ));
        }
        for (key, value) in [
            ("from", query.from),
            ("to", query.to),
            ("step_seconds", query.step_seconds),
        ] {
            if let Some(value) = value {
                values.push((key, value.to_string()));
            }
        }
        let mut request = Request::get(&path);
        request.query = &values;
        request.statuses = vec![StatusCode::OK, StatusCode::ACCEPTED];
        let view: ReportView = self.client.request(request).await?;
        let report = &view.report;
        if view.schema_version != 1
            || view.window != input.window
            || report.scope.admin != self.admin
            || (report.status != "pending" && report.status != "ready")
            || (report.status == "pending"
                && (view.generated_at.is_some()
                    || view.stale
                    || view.refresh.state != RefreshState::Pending
                    || view.refresh.report_id.as_ref() != Some(&report.report_id)))
            || (report.status == "ready"
                && (view.generated_at.is_none()
                    || !report.report_complete
                    || report.summary.is_none()))
        {
            return Err(Error::protocol(
                "usage view selection or completion mismatch",
            ));
        }
        let mut wanted = query.filters.clone();
        wanted.search = wanted.search.trim().to_lowercase();
        wanted.statuses.sort();
        wanted.statuses.dedup();
        if wanted.agent_id.is_some() && wanted.sandbox_id.is_none() {
            wanted.sandbox_id = report.query.filters.sandbox_id.clone();
        }
        let mut actual = report.query.filters.clone();
        actual.statuses.sort();
        let seconds = match input.window {
            ViewWindow::Day => Some(86_400),
            ViewWindow::Week => Some(604_800),
            ViewWindow::Month => Some(2_592_000),
            ViewWindow::Custom => None,
        };
        if wanted != actual
            || query
                .step_seconds
                .is_some_and(|step| step != report.query.step_seconds)
            || seconds.is_some_and(|seconds| {
                report.query.to.checked_sub(report.query.from) != Some(seconds)
            })
            || (input.window == ViewWindow::Custom
                && (query.from != Some(report.query.from) || query.to != Some(report.query.to)))
        {
            return Err(Error::protocol("usage view filters or range mismatch"));
        }
        Ok(view)
    }
}
