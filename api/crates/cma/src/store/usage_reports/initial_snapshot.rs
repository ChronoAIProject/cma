use super::*;
use layer2_core::sandbox_metrics::{Report, ReportStatus};

impl ReportingStore {
    async fn initial_namespace(&self, report: &Report) -> Result<Document> {
        let job = self.collection(JOBS)
            .find_one(doc! {"_id":&report.report_id,"principal":&report.scope.principal,"admin":report.scope.admin})
            .max_time(DB_DEADLINE).await.map_err(unavailable)?.ok_or(MetricsError::NotFound)?;
        let retire_after = match job.get("retire_after") {
            None => None,
            Some(mongodb::bson::Bson::Int64(at)) => Some(*at),
            Some(_) => return Err(MetricsError::Unavailable),
        };
        if report.expires_at <= now() || retire_after.is_some_and(|at| at <= now() as i64) {
            return Err(MetricsError::Expired);
        }
        let retained = match job.get("kind") {
            None => false,
            Some(mongodb::bson::Bson::String(kind)) if kind == views::VIEW_KIND => true,
            _ => return Err(MetricsError::Unavailable),
        };
        if report.status != ReportStatus::Ready
            || !report.report_complete
            || report.summary.is_none()
            || job.get_str("state").ok() != Some(if retained { "view_ready" } else { "ready" })
            || job.get_i64("lease_until").ok() != Some(0)
            || job
                .get_i64("generation")
                .map_or(true, |generation| generation < 1)
            || job.get_i64("logical_expires_at").ok() != Some(report.expires_at as i64)
            || (retained && job.get_i64("retain_until").ok() != Some(report.expires_at as i64))
            || (retained && retire_after.is_none())
            || (!retained && job.contains_key("retain_until"))
            || encode(&decode::<Report>(&job)?)? != encode(report)?
        {
            return Err(MetricsError::Unavailable);
        }
        Ok(job)
    }

    pub(super) async fn verify_initial_snapshot(&self, report: &Report) -> Result<u64> {
        self.check_circuit(now())?;
        let before = self.initial_namespace(report).await?;
        self.retained_counts(report, &before).await?;
        let after = self.initial_namespace(report).await?;
        // Supported writers atomically fence child inserts on the running job.
        // Ready namespaces cannot grow; expiry/retirement can still remove them.
        for field in ["generation", "row_count", "chart_count"] {
            if before.get_i64(field).ok() != after.get_i64(field).ok() {
                return Err(MetricsError::Unavailable);
            }
        }
        u64::try_from(after.get_i64("generation").map_err(unavailable)?).map_err(unavailable)
    }
}
