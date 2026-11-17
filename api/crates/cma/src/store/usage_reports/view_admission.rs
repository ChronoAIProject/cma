//! Admission holds both the old snapshot and its replacement against shared budgets.
use super::*;
use crate::store::transaction_scope as tx;
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
        self.admit_view(scope, selection, at, key).await
    }
    async fn admit_view(
        &self,
        scope: &Scope,
        selection: &ViewSelection,
        at: u64,
        key: &str,
    ) -> Result<Report> {
        self.check_circuit(at)?;
        let report = tx::run(&self.client, (self, scope, selection, at, key), |&(store, scope, selection, at, key), session| {
            Box::pin(async move {
                store.collection(CONTROL)
                    .update_one(doc! {"_id":"admission"}, doc! {"$inc":{"revision":1_i64}})
                    .session(&mut *session)
                    .await
                    .map_err(tx::mongo)?;
                let pointer = store
                    .collection(CONTROL)
                    .find_one(doc! {"_id":key})
                    .session(&mut *session)
                    .await
                    .map_err(tx::mongo)?;
        if let Some(pointer) = &pointer {
            if pointer.get_str("kind").ok() != Some(VIEW_KIND)
                || pointer.get_str("principal").ok() != Some(&scope.principal)
                || pointer.get_bool("admin").ok() != Some(scope.admin)
            {
                return Err(tx::domain(MetricsError::Unavailable));
            }
            if pointer.contains_key("active_id") && pointer.get_str("active_id").is_err() {
                return Err(tx::domain(MetricsError::Unavailable));
            }
            if let Ok(active) = pointer.get_str("active_id") {
                if let Some(job) = store.collection(JOBS).find_one(doc! {"_id":active,"kind":VIEW_KIND,
                    "state":{"$in":["view_pending","view_running"]},"logical_expires_at":{"$gt":at as i64}})
                    .session(&mut *session).await.map_err(tx::mongo)? {
                    let report: Report = decode(&job).map_err(tx::domain)?;
                    if !selection.matches(scope, &report).map_err(tx::domain)? {
                        return Err(tx::domain(MetricsError::Unavailable));
                    }
                    return Ok(report);
                }
            }
        } else {
            let global = store
                .collection(CONTROL)
                .count_documents(doc! {"kind":VIEW_KIND})
                .session(&mut *session)
                .await
                .map_err(tx::mongo)?;
            let tenant = store
                .collection(CONTROL)
                .count_documents(doc! {"kind":VIEW_KIND,"principal":&scope.principal})
                .session(&mut *session)
                .await
                .map_err(tx::mongo)?;
            if global >= 64 || tenant >= 8 {
                return Err(tx::domain(MetricsError::Busy));
            }
        }
        // Count even zero-byte and cleanup tombstones until Mongo actually
        // removes them. Empty reports cannot evade metadata/count bounds.
        let jobs = store
            .collection(JOBS)
            .count_documents(doc! {"kind":VIEW_KIND})
            .session(&mut *session)
            .await
            .map_err(tx::mongo)?;
        let tenant_jobs = store
            .collection(JOBS)
            .count_documents(doc! {"kind":VIEW_KIND,"principal":&scope.principal})
            .session(&mut *session)
            .await
            .map_err(tx::mongo)?;
        if jobs >= 512 || tenant_jobs >= 64 {
            return Err(tx::domain(MetricsError::Busy));
        }
        let mut reservations = store.collection(JOBS).aggregate([
            doc! {"$match":{"reserved":true}},
            doc! {"$group":{"_id":null,"global":{"$sum":"$reserved_bytes"},
                "tenant":{"$sum":{"$cond":[{"$eq":["$principal",{"$literal":&scope.principal}]},"$reserved_bytes",0_i64]}}}},
        ]).max_time(DB_DEADLINE).session(&mut *session).await.map_err(tx::mongo)?;
        let occupied = reservations
            .next(&mut *session)
            .await
            .transpose()
            .map_err(tx::mongo)?;
        let reserved = |key| -> tx::Result<i64> {
            occupied
                .as_ref()
                .map_or(Ok(0), |d| d.get_i64(key).map_err(tx::unavailable))
        };
        let queued = store.collection(JOBS).count_documents(doc! {"state":{"$in":["pending","view_pending"]},"logical_expires_at":{"$gt":at as i64}})
            .session(&mut *session).await.map_err(tx::mongo)?;
        if reserved("global")? > store.budgets.global_bytes - store.budgets.report_bytes
            || reserved("tenant")? > store.budgets.tenant_bytes - store.budgets.report_bytes
            || queued >= store.budgets.queue
        {
            return Err(tx::domain(MetricsError::Busy));
        }
        let revision = pointer
            .as_ref()
            .map_or(Ok(0_i64), |d| d.get_i64("revision").map_err(tx::unavailable))?
            .checked_add(1)
            .ok_or_else(|| tx::domain(MetricsError::Unavailable))?;
        let id = format!("umr_{}", uuid::Uuid::new_v4().simple());
        let retain_until = at
            .checked_add(store.view_policy.retain_seconds)
            .ok_or_else(|| tx::domain(MetricsError::Unavailable))?;
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
        "reserved_bytes":store.budgets.report_bytes,"expires_at":mongodb::bson::Bson::Null,"bytes":0_i64,
        "row_count":0_i64,"chart_count":0_i64,"record":encode(&report).map_err(tx::domain)?};
        store.collection(JOBS)
            .insert_one(job)
            .session(&mut *session)
            .await
            .map_err(tx::mongo)?;
        store.collection(CONTROL).update_one(doc! {"_id":key},doc! {
            "$setOnInsert":{"document_schema_version":super::super::schema::DOCUMENT_SCHEMA_VERSION},
            "$set":{"kind":VIEW_KIND,
            "principal":&scope.principal,"admin":scope.admin,"revision":revision,"active_id":&id,
            "expires_at":expires(retain_until+86_400)}}).upsert(true).session(&mut *session).await.map_err(tx::mongo)?;
                Ok(report)
            })
        }).await?;
        self.notifications.wake();
        Ok(report)
    }
}
