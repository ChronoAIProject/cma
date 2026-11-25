//! Supported writers freeze Ready namespaces. Deletion/expiry must never make
//! ownership validation vacuously authorize residual charts or summary tiles.
use super::*;
use layer2_core::sandbox_metrics::{Report, ReportStatus};
use views::VIEW_KIND;

impl ReportingStore {
    pub(super) async fn retained_namespace(&self, report: &Report) -> Result<Option<Document>> {
        let job = self
            .collection(JOBS)
            .find_one(doc! {"_id":&report.report_id,
            "principal":&report.scope.principal,"admin":report.scope.admin})
            .max_time(DB_DEADLINE)
            .await
            .map_err(unavailable)?
            .ok_or(MetricsError::NotFound)?;
        if job.contains_key(INVALIDATED_FIELD) {
            return Err(MetricsError::Unavailable);
        }
        if job.get_str("kind").ok() != Some(VIEW_KIND) {
            if job.contains_key("retain_until")
                || job
                    .get_str("state")
                    .is_ok_and(|state| state.starts_with("view_"))
            {
                return Err(MetricsError::Unavailable);
            }
            return Ok(None);
        }
        if report.expires_at <= now()
            || job
                .get_i64("retire_after")
                .is_ok_and(|at| at <= now() as i64)
        {
            return Err(MetricsError::Expired);
        }
        if report.status != ReportStatus::Ready
            || !report.report_complete
            || report.summary.is_none()
            || job.get_str("state").ok() != Some("view_ready")
            || job.get_i64("lease_until").ok() != Some(0)
            || job
                .get_i64("generation")
                .map_or(true, |generation| generation < 1)
            || job.get_i64("logical_expires_at").ok() != Some(report.expires_at as i64)
            || job.get_i64("retain_until").ok() != Some(report.expires_at as i64)
            || encode(&decode::<Report>(&job)?)? != encode(report)?
        {
            return Err(MetricsError::Unavailable);
        }
        Ok(Some(job))
    }
    pub(super) async fn retained_counts(&self, report: &Report, job: &Document) -> Result<()> {
        let expected = report
            .summary
            .as_ref()
            .ok_or(MetricsError::Unavailable)?
            .sandbox_count;
        let rows = self
            .collection(ROWS)
            .count_documents(doc! {"report_id":&report.report_id})
            .max_time(DB_DEADLINE)
            .await
            .map_err(unavailable)?;
        let charts = self
            .collection(CHARTS)
            .count_documents(doc! {"report_id":&report.report_id})
            .max_time(DB_DEADLINE)
            .await
            .map_err(unavailable)?;
        if rows != expected
            || i64::try_from(rows).ok() != job.get_i64("row_count").ok()
            || i64::try_from(charts).ok() != job.get_i64("chart_count").ok()
            || charts < rows.saturating_mul(3)
        {
            return Err(MetricsError::Unavailable);
        }
        Ok(())
    }
    pub(super) async fn verify_retained_snapshot(&self, report: &Report) -> Result<()> {
        if report.status != ReportStatus::Ready {
            return Ok(());
        }
        let Some(job) = self.retained_namespace(report).await? else {
            return Ok(());
        };
        self.retained_counts(report, &job).await?;
        let after = self
            .retained_namespace(report)
            .await?
            .ok_or(MetricsError::Unavailable)?;
        if after.get_i64("generation").ok() != job.get_i64("generation").ok() {
            return Err(MetricsError::Unavailable);
        }
        Ok(())
    }
}
