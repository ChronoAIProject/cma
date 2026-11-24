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
            if report.expires_at <= at {
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
        let pipeline = [
            doc! {"$match":{"report_id":&report.report_id}},
            doc! {"$lookup":{"from":states_collection,"localField":"sandbox_id","foreignField":"_id","as":"current"}},
            doc! {"$lookup":{"from":"sandboxes","localField":"sandbox_id","foreignField":"_id","as":"sandbox"}},
            doc! {"$match":{"$expr":{"$or":[
                {"$ne":["$owner",{"$arrayElemAt":["$current.owner",0]}]},
                {"$and":[{"$gt":[{"$size":"$sandbox"},0]},
                    {"$ne":["$owner",{"$arrayElemAt":["$sandbox.owner",0]}]}]}
            ]}}},
            doc! {"$limit":1},
            doc! {"$project":{"_id":1}},
        ];
        let mut cursor = rows
            .aggregate(pipeline)
            .max_time(DB_DEADLINE)
            .await
            .map_err(unavailable)?;
        if cursor.try_next().await.map_err(unavailable)?.is_some() {
            return Err(MetricsError::NotFound);
        }
        Ok(())
    }
}
