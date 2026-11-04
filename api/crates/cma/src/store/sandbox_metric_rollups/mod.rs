//! Versioned hourly chunks of per-minute aggregates, never raw samples.
use super::usage_reports::{decode, encode, expires, unavailable, ReportingStore, DB_DEADLINE};
use layer2_core::sandbox_metrics::{
    MetricRollup, MetricsError, Range, Result, ALGORITHM_VERSION, RETENTION_SECONDS,
};
use mongodb::bson::doc;
pub(crate) const COLLECTION: &str = "sandbox_metric_rollups";
mod live;
pub(crate) use live::LiveReading;
pub(crate) async fn initialize(store: &ReportingStore) -> Result<()> {
    store
        .index(
            COLLECTION,
            doc! {"sandbox_id":1,"day":1,"chunk":1},
            true,
            false,
        )
        .await?;
    Ok(())
}
impl ReportingStore {
    pub(crate) async fn save_metric_rollups(&self, rollups: &[MetricRollup]) -> Result<()> {
        let mut updates = Vec::new();
        for rollup in rollups {
            if rollup.minutes.len() > 60 || rollup.algorithm_version != ALGORITHM_VERSION {
                return Err(MetricsError::Unavailable);
            }
            let key = format!("{}:{}:{}", rollup.sandbox_id, rollup.day, rollup.chunk);
            let d = doc! {"_id":&key,"document_schema_version":1_i64,"sandbox_id":&rollup.sandbox_id,"owner":&rollup.owner_subject,
            "day":rollup.day as i64,"chunk":rollup.chunk as i64,"cutoff":rollup.cutoff as i64,
            "expires_at":expires(rollup.day + (u64::from(rollup.chunk) + 1) * 3600 + RETENTION_SECONDS),"record":encode(rollup)?};
            // Upsert by stable identity, then fence in the update pipeline. A
            // partial retry never erases observations already stored by a
            // successful source read, and an older worker cannot move cutoff back.
            updates.push(doc! {"q":{"_id":key},"u":[{"$replaceWith":{"$cond":[
            {"$or":[{"$eq":[{"$type":"$record"},"missing"]},
                {"$and":[{"$lte":["$cutoff",rollup.cutoff as i64]},rollup.source_complete]}]},
            {"$literal":d},"$$ROOT"]}}],"upsert":true,"multi":false});
        }
        for chunk in updates.chunks(200) {
            self.bulk_updates(COLLECTION, chunk.to_vec()).await?;
        }
        Ok(())
    }
    pub(crate) async fn metric_rollups(
        &self,
        session: &mut mongodb::ClientSession,
        ids: &[String],
        range: Range,
    ) -> Result<Vec<MetricRollup>> {
        let _timer = self.time_read("metric_rollups")?;
        if ids.len() > 200 || range.seconds() > 86_400 {
            return Err(MetricsError::Busy);
        }
        let mut cursor=self.collection(COLLECTION).find(doc!{"sandbox_id":{"$in":ids},"day":{"$gte":(range.from/86_400*86_400) as i64,"$lte":(range.to.saturating_sub(1)/86_400*86_400) as i64}})
            .max_time(DB_DEADLINE).session(&mut *session).await.map_err(unavailable)?;
        let mut out = Vec::new();
        while let Some(d) = cursor
            .next(&mut *session)
            .await
            .transpose()
            .map_err(unavailable)?
        {
            let value: MetricRollup = decode(&d)?;
            if value.schema_version != 1 || value.algorithm_version != ALGORITHM_VERSION {
                return Err(MetricsError::Unavailable);
            }
            out.push(value);
            if out.len() > 4800 {
                return Err(MetricsError::Busy);
            }
        }
        Ok(out)
    }
}
