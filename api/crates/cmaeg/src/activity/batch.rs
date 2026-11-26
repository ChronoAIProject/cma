use super::*;
use cmaeg_core::ids::EventId;

impl ActivityStore {
    /// Apply a bounded callback's facts once per retained daily bucket. All
    /// original event/consumer/message identities remain in their own Change.
    pub(crate) async fn change_batch(
        &self,
        expected_owner: &str,
        record: &str,
        changes: &[(EventId, Change)],
    ) -> Result<(), RecordError> {
        if changes.is_empty() {
            return Ok(());
        }
        if changes.len() > 32 {
            return Err(malformed());
        }
        // As in change(), recover ownership from the sealed channel record.
        let url = self.records.record_url(record)?;
        let value = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            self.records.get(url.as_str()),
        )
        .await
        .map_err(|_| unavailable())??;
        let opened = open_blob(&self.keys, &value.blob).map_err(invalid)?;
        let channel: cmaeg_core::channel::ChannelDefinition =
            serde_json::from_slice(&opened.payload).map_err(invalid)?;
        if channel.record_id.as_str() != record || channel.owner.subject != expected_owner {
            return Err(malformed());
        }
        for (event, change) in changes {
            self.history
                .change(expected_owner, record, event.as_str(), change)
                .await
                .map_err(history_error)?;
        }
        let retained = try_join_all(
            (0..RETENTION_DAYS)
                .map(|offset| self.read(&channel.owner.subject, record, today() - offset)),
        )
        .await?;
        for (bucket, _) in retained.into_iter().flatten() {
            if !bucket.items.iter().any(|row| {
                changes
                    .iter()
                    .any(|(event, _)| row.event_id == event.as_str())
            }) {
                continue;
            }
            self.modify(&channel.owner.subject, record, bucket.day, |bucket| {
                for (event, change) in changes {
                    if let Some(row) = bucket
                        .items
                        .iter_mut()
                        .find(|row| row.event_id == event.as_str())
                    {
                        change.apply(row);
                    }
                }
            })
            .await?;
        }
        Ok(())
    }
}
