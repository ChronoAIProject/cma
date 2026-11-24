use super::*;
use crate::store::transaction_scope as tx;
use layer2_core::sandbox_metrics::{query_hash, Report, ReportQuery, ReportStatus, Scope};
impl ReportingStore {
    pub(super) async fn admit(&self, scope: Scope, query: ReportQuery, at: u64) -> Result<Report> {
        self.check_circuit(at)?;
        let hash = query_hash(&scope, &query, at / 60)?;
        if let Some(d) = self
            .collection(JOBS)
            .find_one(doc! {"query_hash":&hash})
            .max_time(DB_DEADLINE)
            .await
            .map_err(unavailable)?
        {
            let report: Report = decode(&d)?;
            if report.status == ReportStatus::Failed {
                return Err(MetricsError::Unavailable);
            }
            if report.expires_at <= at {
                return Err(MetricsError::Expired);
            }
            return Ok(report);
        }
        self.admit_new(scope, query, at, hash, None).await
    }

    pub(super) async fn admit_new(
        &self,
        scope: Scope,
        query: ReportQuery,
        at: u64,
        hash: String,
        replacement: Option<&Document>,
    ) -> Result<Report> {
        let request = cma_am_reporting::AdmissionRequest {
            scope: &scope,
            query: &query,
            at,
            hash: &hash,
            replacement,
            budget: cma_am_reporting::AdmissionBudget {
                global_bytes: self.budgets.global_bytes,
                tenant_bytes: self.budgets.tenant_bytes,
                report_bytes: self.budgets.report_bytes,
                queue: self.budgets.queue,
            },
        };
        let cma_am_reporting::Admission {
            report,
            reservation,
        } = tx::run(
            &self.client,
            (&self.persistence, &request),
            |&(store, request), session| Box::pin(store.admit_report(request, session)),
        )
        .await?;
        if let Some((queued, global)) = reservation {
            self.notifications.wake();
            tracing::info!(
                report_id = %report.report_id,
                queued_jobs = queued + 1,
                reserved_bytes = global + self.budgets.report_bytes,
                "sandbox report admitted"
            );
        }
        Ok(report)
    }
}
