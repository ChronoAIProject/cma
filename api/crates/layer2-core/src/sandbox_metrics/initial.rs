use super::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct InitialPage {
    pub limit: u16,
    pub sort: Sort,
    pub order: Order,
}
impl Default for InitialPage {
    fn default() -> Self {
        Self {
            limit: 50,
            sort: Sort::default(),
            order: Order::default(),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct InitialProjection {
    pub first_page: InitialPage,
    pub total_metrics: Vec<Metric>,
}
impl InitialProjection {
    pub(super) fn validate(&self, admin: bool) -> Result<()> {
        self.page_query().validate(admin)?;
        if self.first_page.limit > 50
            || self.total_metrics.len() > 3
            || self.total_metrics.iter().collect::<BTreeSet<_>>().len() != self.total_metrics.len()
        {
            return Err(MetricsError::Invalid(
                "Initial views allow at most 50 rows and three distinct total metrics.".into(),
            ));
        }
        Ok(())
    }
    pub(super) fn page_query(&self) -> PageQuery {
        PageQuery {
            limit: Some(self.first_page.limit),
            cursor: None,
            sort: self.first_page.sort,
            order: self.first_page.order,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct InitialData {
    pub projection: InitialProjection,
    pub rows: Rows,
    pub charts: Vec<Series>,
}

#[derive(Debug, Serialize)]
pub struct InitialReport {
    pub report: Report,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initial: Option<InitialData>,
}

pub(super) fn validate_initial_report(report: &Report) -> Result<()> {
    let query = ReportInput {
        from: Some(report.query.from),
        to: Some(report.query.to),
        step_seconds: Some(report.query.step_seconds),
        filters: report.query.filters.clone(),
    }
    .validate(report.as_of)
    .map_err(|_| MetricsError::Unavailable)?;
    if report.schema_version != 1
        || query != report.query
        || report.effective_range != query.effective_range(report.as_of)
        || !report.report_complete
        || report.summary.is_none()
    {
        return Err(MetricsError::Unavailable);
    }
    Ok(())
}

pub(super) fn validate_initial_rows(
    report: &Report,
    page: &InitialPage,
    rows: &Rows,
) -> Result<()> {
    let count = report
        .summary
        .as_ref()
        .ok_or(MetricsError::Unavailable)?
        .sandbox_count;
    if rows.report_id != report.report_id
        || rows.as_of != report.as_of
        || rows.total_count != count
        || rows.items.len() as u64 != count.min(u64::from(page.limit))
        || rows.next.is_some() != (count > u64::from(page.limit))
        || rows
            .next
            .as_ref()
            .is_some_and(|cursor| cursor.is_empty() || cursor.len() > 4096)
        || rows
            .items
            .iter()
            .map(|r| &r.sandbox_id)
            .collect::<BTreeSet<_>>()
            .len()
            != rows.items.len()
        || rows.items.iter().any(|row| {
            !query::opaque_id(&row.sandbox_id, "sbx_")
                || (!report.scope.admin && row.owner.subject != report.scope.principal)
                || report
                    .query
                    .filters
                    .owner
                    .as_ref()
                    .is_some_and(|owner| owner != &row.owner.subject)
                || report
                    .query
                    .filters
                    .sandbox_id
                    .as_ref()
                    .is_some_and(|id| id != &row.sandbox_id)
        })
    {
        return Err(MetricsError::Unavailable);
    }
    Ok(())
}

pub(super) fn validate_initial_series(
    report: &Report,
    metric: Metric,
    series: &Series,
) -> Result<()> {
    let members = report
        .summary
        .as_ref()
        .ok_or(MetricsError::Unavailable)?
        .sandbox_count;
    let range = report.effective_range;
    let step = report.query.step_seconds;
    let points = if range.seconds() == 0 {
        0
    } else {
        (range.to - 1) / step - range.from / step + 1
    };
    if series.report_id != report.report_id
        || series.metric != metric
        || series.unit != metric.unit()
        || series.source != SOURCE
        || points > 2016
        || series.lines.len() != usize::from(members > 0)
        || series.lines.iter().any(|line| {
            line.id != "total"
                || line.points.len() as u64 != points
                || line.points.iter().enumerate().any(|(i, point)| {
                    let start = (range.from / step + i as u64) * step;
                    point.from != start.max(range.from)
                        || point.to != start.saturating_add(step).min(range.to)
                })
        })
    {
        return Err(MetricsError::Unavailable);
    }
    Ok(())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InitialReportInput {
    pub query: ReportInput,
    pub projection: InitialProjection,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InitialViewInput {
    pub selection: ViewInput,
    pub projection: InitialProjection,
}

#[derive(Debug, Serialize)]
pub struct InitialView {
    pub view: ReportView,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initial: Option<InitialData>,
}
