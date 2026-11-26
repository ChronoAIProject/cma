use super::*;
impl ReportingStore {
    pub(crate) async fn save_live(
        &self,
        holder: &str,
        readings: &[(String, LiveReading)],
    ) -> Result<()> {
        let updates = metric_rollups::live_updates(readings)?;
        if !updates.is_empty() {
            self.controlled_updates("rollups", holder, COLLECTION, updates)
                .await?;
        }
        Ok(())
    }
    pub(crate) async fn live_readings(
        &self,
        ids: &[String],
    ) -> Result<std::collections::BTreeMap<String, LiveReading>> {
        let mut session = self.snapshot().await?;
        self.live_readings_at(ids, &mut session).await
    }
    pub(crate) async fn live_readings_at(
        &self,
        ids: &[String],
        session: &mut mongodb::ClientSession,
    ) -> Result<std::collections::BTreeMap<String, LiveReading>> {
        metric_rollups::read_live(self.collection(COLLECTION), ids, session, DB_DEADLINE).await
    }
}
