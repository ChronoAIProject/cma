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
        // Sum each job's admitted reservation, including during a rolling
        // budget change. Counting jobs with this replica's current budget can
        // undercount existing larger reservations or exceed a nonmultiple cap.
        let mut reservations = self.collection(JOBS).aggregate([
            doc! {"$match":{"reserved":true}},
            doc! {"$group":{"_id":null,"global":{"$sum":"$reserved_bytes"},
                "tenant":{"$sum":{"$cond":[{"$eq":["$principal",&scope.principal]},"$reserved_bytes",0_i64]}}}},
        ]).max_time(DB_DEADLINE).session(&mut session).await.map_err(unavailable)?;
        let occupied = reservations
            .next(&mut session)
            .await
            .transpose()
            .map_err(unavailable)?;
        let reserved = |key| -> Result<i64> {
            occupied
                .as_ref()
                .map_or(Ok(0), |d| d.get_i64(key).map_err(unavailable))
        };
        let global = reserved("global")?;
        let tenant = reserved("tenant")?;
        let queued = self
            .collection(JOBS)
            .count_documents(doc! {"state":"pending","logical_expires_at":{"$gt":at as i64}})
            .session(&mut session)
            .await
            .map_err(unavailable)?;
        if global > self.budgets.global_bytes - self.budgets.report_bytes
            || tenant > self.budgets.tenant_bytes - self.budgets.report_bytes
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
        "logical_expires_at":report.expires_at as i64,"reserved":true,"reserved_bytes":self.budgets.report_bytes,
        "expires_at":expires(report.expires_at+86_400),"bytes":0_i64,"record":encode(&report)?};
        self.collection(JOBS)
            .insert_one(d)
            .session(&mut session)
            .await
            .map_err(unavailable)?;
        session.commit_transaction().await.map_err(unavailable)?;
        tracing::info!(
            queued_jobs = queued + 1,
            reserved_bytes = global + self.budgets.report_bytes,
            "sandbox report admitted"
        );
        Ok(report)
    }
}
