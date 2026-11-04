use super::*;
use layer2_core::sandbox_metrics::{
    query_hash, Report, ReportQuery, ReportStatus, Scope, FINALIZATION_DELAY_SECONDS,
    REPORT_TTL_SECONDS,
};
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
        let mut session = self.client.start_session().await.map_err(unavailable)?;
        session.start_transaction().await.map_err(unavailable)?;
        // Serialize admission across replicas without involving Agent sessions.
        self.collection(CONTROL)
            .update_one(doc! {"_id":"admission"}, doc! {"$inc":{"revision":1_i64}})
            .session(&mut session)
            .await
            .map_err(unavailable)?;
        let occupied = doc! {"reserved":true};
        let count = self
            .collection(JOBS)
            .count_documents(occupied.clone())
            .session(&mut session)
            .await
            .map_err(unavailable)?;
        let mut tenant = occupied;
        tenant.insert("principal", &scope.principal);
        let owned = self
            .collection(JOBS)
            .count_documents(tenant)
            .session(&mut session)
            .await
            .map_err(unavailable)?;
        let queued = self
            .collection(JOBS)
            .count_documents(doc! {"state":"pending","logical_expires_at":{"$gt":at as i64}})
            .session(&mut session)
            .await
            .map_err(unavailable)?;
        if count as i64 * self.budgets.report_bytes >= self.budgets.global_bytes
            || owned as i64 * self.budgets.report_bytes >= self.budgets.tenant_bytes
            || queued >= self.budgets.queue
        {
            session.abort_transaction().await.map_err(unavailable)?;
            return Err(MetricsError::Busy);
        }
        let id = format!("umr_{}", uuid::Uuid::new_v4().simple());
        let prefix = if scope.admin {
            "/api/v1/admin/usage/metrics"
        } else {
            "/api/v1/usage/metrics"
        };
        let report = Report {
            schema_version: 1,
            report_id: id.clone(),
            status: ReportStatus::Pending,
            report_complete: false,
            scope: scope.clone(),
            effective_range: query.effective_range(at),
            query,
            as_of: at,
            measurement_cutoff: at.saturating_sub(FINALIZATION_DELAY_SECONDS),
            expires_at: at + REPORT_TTL_SECONDS,
            allocation_watermark: String::new(),
            mapping_revision: 0,
            available_from: None,
            tracking_started_at: None,
            warnings: Vec::new(),
            source_health: "pending".into(),
            summary: None,
            poll_url: format!("{prefix}/reports/{id}"),
            retry_after_seconds: 2,
        };
        let d = doc! {"_id":&id,"document_schema_version":1_i64,"query_hash":hash,"principal":&scope.principal,"admin":scope.admin,
        "state":"pending","created_at":at as i64,"lease_until":0_i64,"lease_holder":"","generation":0_i64,
        "logical_expires_at":report.expires_at as i64,"reserved":true,
        "expires_at":expires(report.expires_at+86_400),"bytes":0_i64,"record":encode(&report)?};
        self.collection(JOBS)
            .insert_one(d)
            .session(&mut session)
            .await
            .map_err(unavailable)?;
        session.commit_transaction().await.map_err(unavailable)?;
        tracing::info!(
            queued_jobs = queued + 1,
            reserved_bytes = (count + 1) * self.budgets.report_bytes as u64,
            "sandbox report admitted"
        );
        Ok(report)
    }
}
