//! Versioned hourly chunks of per-minute aggregates, never raw samples.
use super::usage_reports::{ReportingStore, DB_DEADLINE};
use cma_am_reporting::metric_rollups;
use layer2_core::sandbox_metrics::{MetricRollup, Range, Result};
pub(crate) use metric_rollups::{LiveReading, COLLECTION};
use mongodb::bson::doc;
mod live;
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
    pub(crate) async fn save_metric_rollups(
        &self,
        key: &str,
        holder: &str,
        rollups: &[MetricRollup],
    ) -> Result<()> {
        let updates = metric_rollups::metric_updates(rollups)?;
        for chunk in updates.chunks(200) {
            self.controlled_updates(key, holder, COLLECTION, chunk.to_vec())
                .await?;
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
        metric_rollups::read_metric_rollups(
            self.collection(COLLECTION),
            session,
            ids,
            range,
            DB_DEADLINE,
        )
        .await
    }
}
