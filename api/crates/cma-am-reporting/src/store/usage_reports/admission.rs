//! Persist report admission under the supplied transaction and budget.
use super::{decode, encode};
use crate::{pool::DB_DEADLINE, transaction_scope as tx, ReportingStore};
use layer2_core::sandbox_metrics::{
    MetricsError, Report, ReportQuery, ReportStatus, Scope, FINALIZATION_DELAY_SECONDS,
    REPORT_TTL_SECONDS,
};
use mongodb::{
    bson::{doc, Document},
    ClientSession,
};

pub const JOBS: &str = "sandbox_usage_reports";
pub const CONTROL: &str = "sandbox_usage_report_control";

pub struct AdmissionBudget {
    pub global_bytes: i64,
    pub tenant_bytes: i64,
    pub report_bytes: i64,
    pub queue: u64,
}

pub struct AdmissionRequest<'a> {
    pub scope: &'a Scope,
    pub query: &'a ReportQuery,
    pub at: u64,
    pub hash: &'a str,
    pub replacement: Option<&'a Document>,
    pub budget: AdmissionBudget,
}

pub struct Admission {
    pub report: Report,
    pub reservation: Option<(u64, i64)>,
}

impl ReportingStore {
    pub async fn admit_report(
        &self,
        request: &AdmissionRequest<'_>,
        session: &mut ClientSession,
    ) -> tx::Result<Admission> {
        let AdmissionRequest {
            scope,
            query,
            at,
            hash,
            replacement,
            ..
        } = *request;
        // Serialize admission across replicas without involving Agent sessions.
        self.collection(CONTROL)
            .update_one(doc! {"_id":"admission"}, doc! {"$inc":{"revision":1_i64}})
            .session(&mut *session)
            .await
            .map_err(tx::mongo)?;
        // A concurrent admission may have committed since the first
        // lookup or the previous attempt. Reuse it before charging the
        // reservation again, even when that job filled the budget.
        if replacement.is_none() {
            if let Some(existing) = self
                .collection(JOBS)
                .find_one(doc! {"query_hash":hash})
                .session(&mut *session)
                .await
                .map_err(tx::mongo)?
            {
                let report: Report = decode(&existing)?;
                if report.status == ReportStatus::Failed {
                    return Err(MetricsError::Unavailable.into());
                }
                if report.expires_at <= at
                    || matches!(existing.get_str("state"), Ok("retired" | "view_retired"))
                {
                    return Err(MetricsError::Expired.into());
                }
                return Ok(Admission {
                    report,
                    reservation: None,
                });
            }
        }
        if let Some(previous) = replacement {
            // Release only this exact terminal/expired selection, atomically
            // with the replacement admission. Old descriptors and rows remain
            // addressable under their original report identity and TTL.
            let id = previous.get_str("_id").map_err(tx::unavailable)?;
            let expected = doc! {
                "_id": id,
                "query_hash": hash,
                "generation": previous.get_i64("generation").map_err(tx::unavailable)?,
                "state": previous.get_str("state").map_err(tx::unavailable)?,
                "logical_expires_at": previous.get_i64("logical_expires_at").map_err(tx::unavailable)?,
            };
            let changed = self
                .collection(JOBS)
                .update_one(
                    expected,
                    doc! {"$set": {"query_hash": format!("replaced-selection:{id}")}},
                )
                .session(&mut *session)
                .await
                .map_err(tx::mongo)?;
            if changed.matched_count != 1 {
                return Err(tx::domain(MetricsError::Unavailable));
            }
        }
        // Sum each job's admitted reservation, including during a rolling
        // budget change. Counting jobs with this replica's current budget can
        // undercount existing larger reservations or exceed a nonmultiple cap.
        let mut reservations = self
            .collection(JOBS)
            .aggregate([
                doc! {"$match": {"reserved": true}},
                doc! {"$group": {
                    "_id": null,
                    "global": {"$sum": "$reserved_bytes"},
                    "tenant": {"$sum": {"$cond": [
                        {"$eq": ["$principal", {"$literal": &scope.principal}]},
                        "$reserved_bytes",
                        0_i64
                    ]}}
                }},
            ])
            .max_time(DB_DEADLINE)
            .session(&mut *session)
            .await
            .map_err(tx::mongo)?;
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
        let global = reserved("global")?;
        let tenant = reserved("tenant")?;
        let queued = self
            .collection(JOBS)
            .count_documents(doc! {
                "state": {"$in": ["pending", "view_pending"]},
                "logical_expires_at": {"$gt": at as i64},
            })
            .session(&mut *session)
            .await
            .map_err(tx::mongo)?;
        if global > request.budget.global_bytes - request.budget.report_bytes
            || tenant > request.budget.tenant_bytes - request.budget.report_bytes
            || queued >= request.budget.queue
        {
            tracing::warn!(
                target: "cma::store::usage_reports::admission",
                global_bytes = global,
                tenant_bytes = tenant,
                queued_jobs = queued,
                global_limit = request.budget.global_bytes,
                tenant_limit = request.budget.tenant_bytes,
                queue_limit = request.budget.queue,
                "sandbox report admission budget exhausted"
            );
            return Err(tx::domain(MetricsError::Busy));
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
            query: query.clone(),
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
        "logical_expires_at":report.expires_at as i64,"reserved":true,"reserved_bytes":request.budget.report_bytes,
        "expires_at":mongodb::bson::Bson::Null,"bytes":0_i64,"record":encode(&report).map_err(tx::domain)?};
        self.collection(JOBS)
            .insert_one(d)
            .session(&mut *session)
            .await
            .map_err(tx::mongo)?;
        Ok(Admission {
            report,
            reservation: Some((queued, global)),
        })
    }
}
