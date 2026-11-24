//! Replayable allocation-day projection. The allocated ledger is read-only.
use super::usage_reports::{
    allocation_operation, unavailable, ReportingStore, ALLOCATION_DB_DEADLINE,
};
use cma_am_reporting::usage_rollups::{cache_update, match_cached_rollup};
use layer2_core::{
    sandbox_metrics::{allocation, MetricsError, Range, Result},
    sandbox_usage::SandboxUsage,
};
use mongodb::bson::doc;
mod cold_range;
mod source;
pub(crate) use cma_am_reporting::usage_rollups::COLLECTION;
pub(crate) use source::AllocationDelta;
use std::collections::BTreeMap;
pub(crate) async fn initialize(store: &ReportingStore) -> Result<()> {
    store
        .index(COLLECTION, doc! {"sandbox_id":1,"day":1}, true, false)
        .await?;
    Ok(())
}
impl ReportingStore {
    pub(crate) async fn allocation_periods(
        &self,
        session: &mut mongodb::ClientSession,
        states: &[SandboxUsage],
        range: Range,
    ) -> Result<BTreeMap<String, allocation::Projection>> {
        allocation_operation(self.allocation_periods_inner(session, states, range)).await
    }

    async fn allocation_periods_inner(
        &self,
        session: &mut mongodb::ClientSession,
        states: &[SandboxUsage],
        range: Range,
    ) -> Result<BTreeMap<String, allocation::Projection>> {
        let _timer = self.time_read("allocation_rollups")?;
        let ids: Vec<_> = states.iter().map(|s| s.sandbox_id.as_str()).collect();
        let mut cached = cma_am_reporting::usage_rollups::read_rollups(
            self.collection(COLLECTION),
            session,
            states,
            &ids,
            range,
            ALLOCATION_DB_DEADLINE,
        )
        .await?;
        let missing: Vec<_> = ids
            .into_iter()
            .filter(|id| !cached.contains_key(*id))
            .collect();
        if missing.is_empty() {
            return Ok(cached);
        }
        let events = self.allocation_events(session, &missing, range).await?;
        let mut updates = Vec::new();
        for s in states
            .iter()
            .filter(|s| missing.contains(&s.sandbox_id.as_str()))
        {
            let p = allocation::project(
                s,
                events
                    .get(s.sandbox_id.as_str())
                    .map(Vec::as_slice)
                    .unwrap_or_default(),
                range,
            );
            updates.push(cache_update(s, range, &p)?);
            cached.insert(s.sandbox_id.to_string(), p);
        }
        if !updates.is_empty() {
            self.allocation_cache_updates(updates).await?;
        }
        Ok(cached)
    }
}
