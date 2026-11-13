//! Admission holds both the old snapshot and its replacement against shared budgets.
use super::*;
use layer2_core::sandbox_metrics::{
    Report, ReportStatus, Scope, ViewSelection, FINALIZATION_DELAY_SECONDS,
};
use views::VIEW_KIND;

impl ReportingStore {
    pub(super) async fn enqueue_view(
        &self,
        scope: &Scope,
        selection: &ViewSelection,
        at: u64,
        key: &str,
    ) -> Result<Report> {
        let mut result = Err(MetricsError::Unavailable);
        // Same bounded transaction-conflict opportunity as ordinary admission.
        for attempt in 0..3 {
            result = self.admit_view(scope, selection, at, key).await;
            if !matches!(result, Err(MetricsError::Unavailable)) {
                return result;
            }
            if attempt < 2 {
                tokio::time::sleep(Duration::from_millis(25 * (attempt + 1))).await;
            }
        }
        result
    }
    async fn admit_view(
        &self,
        scope: &Scope,
        selection: &ViewSelection,
        at: u64,
        key: &str,
    ) -> Result<Report> {
        self.check_circuit(at)?;
        let mut session = self.client.start_session().await.map_err(unavailable)?;
        session.start_transaction().await.map_err(unavailable)?;
        self.collection(CONTROL)
            .update_one(doc! {"_id":"admission"}, doc! {"$inc":{"revision":1_i64}})
            .session(&mut session)
            .await
            .map_err(unavailable)?;
        let pointer = self
            .collection(CONTROL)
            .find_one(doc! {"_id":key})
            .session(&mut session)
            .await
            .map_err(unavailable)?;
        if let Some(pointer) = &pointer {
            if pointer.get_str("kind").ok() != Some(VIEW_KIND)
                || pointer.get_str("principal").ok() != Some(&scope.principal)
                || pointer.get_bool("admin").ok() != Some(scope.admin)
            {
                return Err(MetricsError::Unavailable);
            }
            if pointer.contains_key("active_id") && pointer.get_str("active_id").is_err() {
                return Err(MetricsError::Unavailable);
            }
            if let Ok(active) = pointer.get_str("active_id") {
                if let Some(job) = self.collection(JOBS).find_one(doc! {"_id":active,"kind":VIEW_KIND,
                    "state":{"$in":["view_pending","view_running"]},"logical_expires_at":{"$gt":at as i64}})
                    .session(&mut session).await.map_err(unavailable)? {
                    let report: Report = decode(&job)?;
                    if !selection.matches(scope, &report)? { return Err(MetricsError::Unavailable); }
                    return Ok(report);
                }
            }
        } else {
            let global = self
                .collection(CONTROL)
                .count_documents(doc! {"kind":VIEW_KIND})
                .session(&mut session)
                .await
                .map_err(unavailable)?;
            let tenant = self
                .collection(CONTROL)
                .count_documents(doc! {"kind":VIEW_KIND,"principal":&scope.principal})
                .session(&mut session)
                .await
                .map_err(unavailable)?;
            if global >= 64 || tenant >= 8 {
                return Err(MetricsError::Busy);
            }
        }
        // Count even zero-byte and cleanup tombstones until Mongo actually
        // removes them. Empty reports cannot evade metadata/count bounds.
        let jobs = self
            .collection(JOBS)
            .count_documents(doc! {"kind":VIEW_KIND})
            .session(&mut session)
            .await
            .map_err(unavailable)?;
        let tenant_jobs = self
            .collection(JOBS)
            .count_documents(doc! {"kind":VIEW_KIND,"principal":&scope.principal})
            .session(&mut session)
            .await
            .map_err(unavailable)?;
        if jobs >= 512 || tenant_jobs >= 64 {
            return Err(MetricsError::Busy);
        }
        let mut reservations = self.collection(JOBS).aggregate([
            doc! {"$match":{"reserved":true}},
            doc! {"$group":{"_id":null,"global":{"$sum":"$reserved_bytes"},
                "tenant":{"$sum":{"$cond":[{"$eq":["$principal",{"$literal":&scope.principal}]},"$reserved_bytes",0_i64]}}}},
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
        let queued = self.collection(JOBS).count_documents(doc! {"state":{"$in":["pending","view_pending"]},"logical_expires_at":{"$gt":at as i64}})
            .session(&mut session).await.map_err(unavailable)?;
        if reserved("global")? > self.budgets.global_bytes - self.budgets.report_bytes
            || reserved("tenant")? > self.budgets.tenant_bytes - self.budgets.report_bytes
            || queued >= self.budgets.queue
        {
            return Err(MetricsError::Busy);
        }
        let revision = pointer
            .as_ref()
            .map_or(Ok(0_i64), |d| d.get_i64("revision").map_err(unavailable))?
            .checked_add(1)
            .ok_or(MetricsError::Unavailable)?;
        let id = format!("umr_{}", uuid::Uuid::new_v4().simple());
        let retain_until = at
            .checked_add(self.view_policy.retain_seconds)
            .ok_or(MetricsError::Unavailable)?;
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
            query: selection.query.clone(),
            effective_range: selection.query.effective_range(at),
            as_of: at,
            measurement_cutoff: at.saturating_sub(FINALIZATION_DELAY_SECONDS),
            expires_at: retain_until,
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
        let job = doc! {"_id":&id,"document_schema_version":1_i64,"kind":VIEW_KIND,"view_key":key,"view_revision":revision,
        "query_hash":format!("{key}:{revision}"),"principal":&scope.principal,"admin":scope.admin,
        "state":"view_pending","created_at":at as i64,"lease_until":0_i64,"lease_holder":"","generation":0_i64,
        "logical_expires_at":retain_until as i64,"retain_until":retain_until as i64,"reserved":true,
        "reserved_bytes":self.budgets.report_bytes,"expires_at":mongodb::bson::Bson::Null,"bytes":0_i64,
        "row_count":0_i64,"chart_count":0_i64,"record":encode(&report)?};
        self.collection(JOBS)
            .insert_one(job)
            .session(&mut session)
            .await
            .map_err(unavailable)?;
        self.collection(CONTROL).update_one(doc! {"_id":key},doc! {"$set":{"kind":VIEW_KIND,
            "principal":&scope.principal,"admin":scope.admin,"revision":revision,"active_id":&id,
            "expires_at":expires(retain_until+86_400)}}).upsert(true).session(&mut session).await.map_err(unavailable)?;
        session.commit_transaction().await.map_err(unavailable)?;
        self.notifications.wake();
        Ok(report)
    }
}
