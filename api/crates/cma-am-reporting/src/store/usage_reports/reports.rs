//! Scope-bound retained report and membership reads.
use super::{decode, unavailable, JOBS};
use crate::{pool::DB_DEADLINE, ReportingStore};
use futures_util::TryStreamExt;
use layer2_core::sandbox_metrics::{MetricsError, Report, ReportStatus, Result, Scope};
use mongodb::{
    bson::{doc, Document},
    Collection,
};

impl ReportingStore {
    pub async fn read_report(&self, scope: &Scope, id: &str, at: u64) -> Result<Report> {
        let d = self
            .collection(JOBS)
            .find_one(doc! {"_id":id,"principal":&scope.principal,"admin":scope.admin})
            .max_time(DB_DEADLINE)
            .await
            .map_err(unavailable)?
            .ok_or(MetricsError::NotFound)?;
        let report: Report = decode(&d)?;
        if report.expires_at <= at
            || matches!(d.get_str("state"), Ok("retired" | "view_retired"))
            || d.get_i64("retire_after")
                .is_ok_and(|until| until <= at as i64)
        {
            return Err(MetricsError::Expired);
        }
        if report.status == ReportStatus::Failed {
            return Err(MetricsError::Unavailable);
        }
        Ok(report)
    }

    pub async fn read_admitted_report(&self, hash: &str, at: u64) -> Result<Option<Report>> {
        if let Some(d) = self
            .collection(JOBS)
            .find_one(doc! {"query_hash":hash})
            .max_time(DB_DEADLINE)
            .await
            .map_err(unavailable)?
        {
            let report: Report = decode(&d)?;
            if report.status == ReportStatus::Failed {
                return Err(MetricsError::Unavailable);
            }
            if report.expires_at <= at
                || matches!(d.get_str("state"), Ok("retired" | "view_retired"))
            {
                return Err(MetricsError::Expired);
            }
            return Ok(Some(report));
        }
        Ok(None)
    }

    pub async fn check_report_membership(
        &self,
        rows: Collection<Document>,
        states_collection: &str,
        report: &Report,
    ) -> Result<()> {
        // Even admin cached membership must not retain another owner's old
        // attribution after a transfer. Owner changes invalidate the snapshot.
        let pipeline = report_membership_pipeline(&report.report_id, states_collection);
        let started = std::time::Instant::now();
        let mismatch: Result<bool> = async {
            let mut cursor = rows
                .aggregate(pipeline)
                .max_time(DB_DEADLINE)
                .await
                .map_err(unavailable)?;
            Ok(cursor.try_next().await.map_err(unavailable)?.is_some())
        }
        .await;
        tracing::debug!(
            target: "cma::store::usage_reports::reads",
            operation = "report_ownership",
            elapsed_ms = started.elapsed().as_millis() as u64,
            outcome = match &mismatch {
                Ok(false) => "matched",
                Ok(true) => "mismatch",
                Err(_) => "unavailable",
            },
            "sandbox report live ownership check"
        );
        if mismatch? {
            return Err(MetricsError::NotFound);
        }
        Ok(())
    }
}

/// Builds the live membership query used by report authorization and query-plan tests.
pub fn report_membership_pipeline(report_id: &str, states_collection: &str) -> [Document; 8] {
    [
        doc! {"$match":{"report_id":report_id}},
        doc! {"$project":{"_id":1,"sandbox_id":1,"owner":1}},
        doc! {"$lookup":{"from":states_collection,"localField":"sandbox_id","foreignField":"_id","as":"current"}},
        doc! {"$lookup":{"from":"sandboxes","localField":"sandbox_id","foreignField":"_id","as":"sandbox"}},
        // Project after both equality joins to preserve both indexed SBE lookups.
        // Keep the Sandbox array: [{}] must still differ from a deleted Sandbox [].
        doc! {"$project":{"_id":1,"owner":1,"current.owner":1,"sandbox.owner":1}},
        doc! {"$match":{"$expr":{"$or":[
            {"$ne":["$owner",{"$arrayElemAt":["$current.owner",0]}]},
            {"$and":[{"$gt":[{"$size":"$sandbox"},0]},
                {"$ne":["$owner",{"$arrayElemAt":["$sandbox.owner",0]}]}]}
        ]}}},
        doc! {"$limit":1},
        doc! {"$project":{"_id":1}},
    ]
}
