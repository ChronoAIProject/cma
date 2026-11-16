//! Unlisted selections use ordinary bounded jobs, without retained pointers.
use super::*;
use layer2_core::sandbox_metrics::{
    RefreshState, Report, ReportStatus, ReportView, Scope, ViewRefresh, ViewSelection,
    REPORT_TTL_SECONDS,
};

impl ReportingStore {
    pub(super) async fn on_demand_view(
        &self,
        scope: &Scope,
        selection: &ViewSelection,
        at: u64,
    ) -> Result<ReportView> {
        self.check_circuit(at)?;
        // Rolling range boundaries change each minute. The selection identity
        // must survive polling and the subsequent latest read on any replica.
        let key = format!("on-demand:{}", selection.key(scope)?);
        for attempt in 0..3 {
            let report = match self.on_demand_report(scope, selection, at, &key).await {
                Err(MetricsError::Unavailable) if attempt < 2 => {
                    tokio::time::sleep(Duration::from_millis(25 << attempt)).await;
                    continue;
                }
                result => result?,
            };
            let generated_at = match report.status {
                // Ordinary completion atomically sets expires_at to completion
                // time + TTL. Claim/admission as_of is not completion time.
                ReportStatus::Ready => Some(
                    report
                        .expires_at
                        .checked_sub(REPORT_TTL_SECONDS)
                        .ok_or(MetricsError::Unavailable)?,
                ),
                ReportStatus::Pending => None,
                ReportStatus::Failed => return Err(MetricsError::Unavailable),
            };
            return Ok(ReportView {
                schema_version: 1,
                window: selection.window,
                requested_range: selection.query.range(),
                generated_at,
                stale: generated_at.is_some_and(|completed| {
                    at.saturating_sub(completed) >= self.view_policy.freshness_seconds
                }),
                refresh: ViewRefresh {
                    state: if generated_at.is_some() {
                        RefreshState::Idle
                    } else {
                        RefreshState::Pending
                    },
                    report_id: generated_at.is_none().then(|| report.report_id.clone()),
                    retry_after_seconds: if generated_at.is_some() { 0 } else { 2 },
                },
                report,
            });
        }
        Err(MetricsError::Unavailable)
    }

    async fn on_demand_report(
        &self,
        scope: &Scope,
        selection: &ViewSelection,
        at: u64,
        key: &str,
    ) -> Result<Report> {
        self.check_circuit(at)?;
        let previous = self
            .collection(JOBS)
            .find_one(doc! {"query_hash":key})
            .max_time(DB_DEADLINE)
            .await
            .map_err(unavailable)?;
        if let Some(document) = &previous {
            let report: Report = decode(document)?;
            if !selection.matches(scope, &report)?
                || document.get_str("principal").ok() != Some(&scope.principal)
                || document.get_bool("admin").ok() != Some(scope.admin)
                || document.contains_key("kind")
            {
                return Err(MetricsError::Unavailable);
            }
            if report.expires_at > at {
                match report.status {
                    ReportStatus::Pending => return Ok(report),
                    ReportStatus::Ready if !selection.refresh => return Ok(report),
                    // Observing failure never silently starts another job.
                    // A deliberate refresh may replace it; its old ID remains
                    // terminal and cannot become the new attempt.
                    ReportStatus::Failed if !selection.refresh => {
                        return Err(MetricsError::Unavailable);
                    }
                    _ => {}
                }
            }
        }
        self.admit_new(
            scope.clone(),
            selection.query.clone(),
            at,
            key.into(),
            previous.as_ref(),
        )
        .await
    }
}
