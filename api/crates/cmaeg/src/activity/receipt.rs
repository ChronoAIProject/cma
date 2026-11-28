use super::*;

impl ActivityStore {
    pub(in crate::activity) async fn persist_receipt(
        &self,
        owner: &str,
        record: &str,
        row: ChannelActivity,
        received: time::OffsetDateTime,
    ) -> Result<(), RecordError> {
        let _guard = self.writes[stripe(record)].lock().await;
        let mut day = received.unix_timestamp().div_euclid(86400);
        if day < today() - RETENTION_DAYS + 1 {
            return Err(malformed());
        }
        // Source retries may cross midnight. Find the retained first receipt
        // before choosing a daily bucket, so replay never looks newly received.
        let retained = try_join_all(
            (0..RETENTION_DAYS).map(|offset| self.read(owner, record, today() - offset)),
        )
        .await?;
        let legacy = retained.into_iter().flatten().find_map(|(bucket, _)| {
            let existing = bucket
                .items
                .into_iter()
                .find(|item| item.event_id == row.event_id)?;
            day = bucket.day;
            Some(existing)
        });
        if self.history.capturing() {
            if let Some(original) = legacy {
                self.history
                    .import_legacy(owner, record, original, (day + RETENTION_DAYS) * 86_400_000)
                    .await
                    .map_err(history_error)?;
            } else {
                self.history
                    .received(owner, record, row.clone())
                    .await
                    .map_err(history_error)?;
            }
        }
        let event_id = row.event_id.clone();
        self.modify_inner(owner, record, day, move |bucket| {
            if let Some(existing) = bucket.items.iter_mut().find(|r| r.event_id == row.event_id) {
                for consumer in &row.consumers {
                    if !existing
                        .consumers
                        .iter()
                        .any(|c| c.consumer_id == consumer.consumer_id)
                    {
                        existing.consumers.push(consumer.clone());
                    }
                }
            } else {
                bucket.items.push(row.clone());
            }
        })
        .await?;
        let mut contexts = self.contexts.lock().unwrap();
        if contexts.len() >= 4096 {
            contexts.pop_first();
        }
        contexts.insert((record.into(), event_id), (owner.into(), day));
        Ok(())
    }
    pub(in crate::activity) async fn confirms_receipt(
        &self,
        owner: &str,
        record: &str,
        receipt: &ChannelActivity,
    ) -> Result<bool, RecordError> {
        let buckets = try_join_all(
            (0..RETENTION_DAYS).map(|offset| self.read(owner, record, today() - offset)),
        )
        .await?;
        Ok(buckets
            .into_iter()
            .flatten()
            .flat_map(|(bucket, _)| bucket.items)
            .any(|row| {
                row.event_id == receipt.event_id
                    && row.source_refusal == receipt.source_refusal
                    && row.consumers.is_empty()
            }))
    }
}
