//! Shared strict ledger reader for foreground fallback and background buckets.
use super::*;
use layer2_core::sandbox_usage::UsageEvent;
impl ReportingStore {
    pub(crate) async fn allocation_events(
        &self,
        session: &mut mongodb::ClientSession,
        ids: &[&str],
        range: Range,
    ) -> Result<BTreeMap<String, Vec<UsageEvent>>> {
        if ids.len() > 200 || range.seconds() > 86400 {
            return Err(MetricsError::Busy);
        }
        if ids.is_empty() {
            return Ok(BTreeMap::new());
        }
        let mut events = BTreeMap::<String, Vec<UsageEvent>>::new();
        let mut cursor=self.collection(super::super::sandbox_usage::EVENTS).find(doc!{"sandbox_id":{"$in":ids},"ended_at":{"$gt":range.from as i64},"started_at":{"$lt":range.to as i64}})
            .sort(doc!{"sandbox_id":1,"revision":1}).batch_size(2_000).max_time(DB_DEADLINE).session(&mut *session).await.map_err(unavailable)?;
        // The complete bounded day is already retained for exact revision-order
        // projection. Avoid a network round trip for every 200 small events;
        // Mongo still caps each reply at 16 MiB and the 100k event cap is unchanged.
        let mut count = 0;
        let mut bytes = 0usize;
        while let Some(d) = cursor
            .next(&mut *session)
            .await
            .transpose()
            .map_err(unavailable)?
        {
            count += 1;
            bytes = bytes
                .checked_add(
                    mongodb::bson::serialize_to_vec(&d)
                        .map_err(unavailable)?
                        .len(),
                )
                .ok_or(MetricsError::Busy)?;
            if count > 100_000 || bytes > 64 * 1024 * 1024 {
                return Err(MetricsError::Busy);
            }
            let e = super::super::sandbox_usage::read_event(&d).map_err(unavailable)?;
            events.entry(e.sandbox_id.to_string()).or_default().push(e);
        }
        Ok(events)
    }
}
