//! Shared strict ledger reader for foreground fallback and background buckets.
use super::*;
use layer2_core::sandbox_usage::UsageEvent;
#[cfg(test)]
mod tests;
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
        self.read_allocation_events(session, doc! {"sandbox_id":{"$in":ids}}, range, false)
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
        self.read_allocation_events(session, doc! {"$or":clauses}, range, true)
            .await
    }

    async fn read_allocation_events(
        &self,
        session: &mut mongodb::ClientSession,
        filter: mongodb::bson::Document,
        range: Range,
        revision_seek: bool,
    ) -> Result<BTreeMap<String, Vec<UsageEvent>>> {
        allocation_operation(self.read_allocation_events_inner(
            session,
            filter,
            range,
            revision_seek,
        ))
        .await
    }

    async fn read_allocation_events_inner(
        &self,
        session: &mut mongodb::ClientSession,
        mut filter: mongodb::bson::Document,
        range: Range,
        revision_seek: bool,
    ) -> Result<BTreeMap<String, Vec<UsageEvent>>> {
        filter.extend(
            doc! {"ended_at":{"$gt":range.from as i64},"started_at":{"$lt":range.to as i64}},
        );
        let collection = self.collection(super::super::sandbox_usage::EVENTS);
        let find = collection.find(filter);
        // Let full interval reads use the temporal index without a server sort.
        // The incremental path retains its existing revision-bound index seek.
        let find = if revision_seek {
            find.sort(doc! {"sandbox_id":1,"revision":1})
        } else {
            find
        };
        let mut cursor = find
            .batch_size(2_000)
            .max_time(ALLOCATION_DB_DEADLINE)
            .session(&mut *session)
            .await
            .map_err(unavailable)?;
        // The complete bounded day is already retained for exact revision-order
        // projection. Avoid a network round trip for every 200 small events;
        // Mongo still caps each reply at 16 MiB and the 100k event cap is unchanged.
        let mut events = EventBatch::default();
        while cursor.advance(&mut *session).await.map_err(unavailable)? {
            events.push(cursor.current())?;
        }
        // No partially fetched or invalid day reaches projection.
        Ok(events.finish(revision_seek))
    }
}

#[derive(Default)]
struct EventBatch {
    events: BTreeMap<String, Vec<UsageEvent>>,
    count: usize,
    bytes: usize,
}

impl EventBatch {
    fn push(&mut self, document: &mongodb::bson::RawDocument) -> Result<()> {
        self.count += 1;
        self.bytes = self
            .bytes
            .checked_add(document.as_bytes().len())
            .ok_or(MetricsError::Busy)?;
        if self.count > 100_000 || self.bytes > 64 * 1024 * 1024 {
            return Err(MetricsError::Busy);
        }
        let event = super::super::sandbox_usage::raw_event::read(document).map_err(unavailable)?;
        self.events
            .entry(event.sandbox_id.to_string())
            .or_default()
            .push(event);
        Ok(())
    }

    fn finish(mut self, revision_seek: bool) -> BTreeMap<String, Vec<UsageEvent>> {
        if !revision_seek {
            for events in self.events.values_mut() {
                events.sort_unstable_by_key(|event| event.revision);
            }
        }
        self.events
    }
}
