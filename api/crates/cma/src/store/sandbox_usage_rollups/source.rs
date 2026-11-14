//! Shared strict ledger reader for foreground fallback and background buckets.
use super::*;
use layer2_core::sandbox_usage::UsageEvent;
pub(crate) struct AllocationDelta<'a> {
    pub sandbox_id: &'a str,
    pub after_revision: u64,
    pub through_revision: u64,
}
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
        self.read_allocation_events(session, doc! {"sandbox_id":{"$in":ids}}, range)
            .await
    }

    pub(crate) async fn allocation_events_after(
        &self,
        session: &mut mongodb::ClientSession,
        deltas: &[AllocationDelta<'_>],
        range: Range,
    ) -> Result<BTreeMap<String, Vec<UsageEvent>>> {
        if deltas.len() > 200 || range.seconds() > 86400 || range.from >= range.to {
            return Err(MetricsError::Busy);
        }
        if deltas.is_empty() {
            return Ok(BTreeMap::new());
        }
        let mut clauses = Vec::with_capacity(deltas.len());
        for delta in deltas {
            if delta.after_revision >= delta.through_revision
                || delta.through_revision > i64::MAX as u64
            {
                return Err(MetricsError::Unavailable);
            }
            clauses.push(doc! {"sandbox_id":delta.sandbox_id,"revision":{
                "$gt":delta.after_revision as i64,"$lte":delta.through_revision as i64,
            }});
        }
        // The existing (sandbox_id,revision) index seeks directly into each
        // changed tail instead of scanning the already prepared current day.
        self.read_allocation_events(session, doc! {"$or":clauses}, range)
            .await
    }

    async fn read_allocation_events(
        &self,
        session: &mut mongodb::ClientSession,
        mut filter: mongodb::bson::Document,
        range: Range,
    ) -> Result<BTreeMap<String, Vec<UsageEvent>>> {
        filter.extend(
            doc! {"ended_at":{"$gt":range.from as i64},"started_at":{"$lt":range.to as i64}},
        );
        let mut events = BTreeMap::<String, Vec<UsageEvent>>::new();
        let mut cursor = self
            .collection(super::super::sandbox_usage::EVENTS)
            .find(filter)
            .sort(doc! {"sandbox_id":1,"revision":1})
            .batch_size(2_000)
            .max_time(DB_DEADLINE)
            .session(&mut *session)
            .await
            .map_err(unavailable)?;
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
