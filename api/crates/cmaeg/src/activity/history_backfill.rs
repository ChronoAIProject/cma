use super::*;
impl ActivityStore {
    /// Explicit migration for one sealed, current channel generation. Nothing
    /// here launches a provider, updates the recent cache, or attests coverage.
    pub async fn backfill_history(&self, owner: &str, record: &str) -> Result<usize, RecordError> {
        let value = self
            .records
            .get(self.records.record_url(record)?.as_str())
            .await?;
        let opened = open_blob(&self.keys, &value.blob).map_err(invalid)?;
        let channel: cmaeg_core::channel::ChannelDefinition =
            serde_json::from_slice(&opened.payload).map_err(invalid)?;
        if channel.record_id.as_str() != record || channel.owner.subject != owner {
            return Err(RecordError::NotFound);
        }
        let mut count = 0;
        for offset in 0..RETENTION_DAYS {
            let day = today() - offset;
            if let Some((bucket, _)) = self.read(owner, record, day).await? {
                for row in bucket.items {
                    self.history
                        .import_legacy(owner, record, row, (day + RETENTION_DAYS) * 86_400_000)
                        .await
                        .map_err(history_error)?;
                    count += 1;
                }
            }
        }
        Ok(count)
    }
}
