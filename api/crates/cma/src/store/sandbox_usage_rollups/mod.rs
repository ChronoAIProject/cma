//! Replayable allocation-day projection. The allocated ledger is read-only.
use super::usage_reports::{
    allocation_operation, decode, encode, expires, unavailable, ReportingStore,
    ALLOCATION_DB_DEADLINE,
};
use layer2_core::{
    sandbox_metrics::{allocation, MetricsError, Range, Result},
    sandbox_usage::SandboxUsage,
};
use mongodb::bson::doc;
use serde::{Deserialize, Serialize};
mod cold_range;
mod source;
pub(crate) use source::AllocationDelta;
use std::collections::BTreeMap;
pub(crate) const COLLECTION: &str = "sandbox_usage_rollups";
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Rollup {
    schema_version: u32,
    sandbox_id: String,
    owner: String,
    day: u64,
    source_watermark: u64,
    range: Range,
    projection: allocation::Projection,
}
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
        if states.len() > 200 || range.seconds() > 86_400 {
            return Err(MetricsError::Busy);
        }
        let ids: Vec<_> = states.iter().map(|s| s.sandbox_id.as_str()).collect();
        let day = range.from / 86_400 * 86_400;
        let mut cached = BTreeMap::new();
        let mut cursor = self
            .collection(COLLECTION)
            .find(doc! {"sandbox_id":{"$in":&ids},"day":day as i64})
            .max_time(ALLOCATION_DB_DEADLINE)
            .session(&mut *session)
            .await
            .map_err(unavailable)?;
        while let Some(d) = cursor
            .next(&mut *session)
            .await
            .transpose()
            .map_err(unavailable)?
        {
            let r: Rollup = decode(&d)?;
            if states.iter().any(|s| valid_rollup(s, range, day, &r)) {
                cached.insert(r.sandbox_id, r.projection);
            }
        }
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

fn valid_rollup(state: &SandboxUsage, range: Range, day: u64, rollup: &Rollup) -> bool {
    rollup.schema_version == 1
        && rollup.day == day
        && rollup.range == range
        && state.sandbox_id.as_str() == rollup.sandbox_id
        && state.owner.as_str() == rollup.owner
        && (state.revision == rollup.source_watermark
            || (state.revision > rollup.source_watermark
                && rollup.projection.period.complete
                && rollup.projection.period.projected_tail_seconds == 0))
}

fn cache_update(
    state: &SandboxUsage,
    range: Range,
    projection: &allocation::Projection,
) -> Result<mongodb::bson::Document> {
    let day = range.from / 86_400 * 86_400;
    let record = Rollup {
        schema_version: 1,
        sandbox_id: state.sandbox_id.to_string(),
        owner: state.owner.to_string(),
        day,
        source_watermark: state.revision,
        range,
        projection: projection.clone(),
    };
    let key = format!("{}:{day}", state.sandbox_id);
    Ok(
        doc! {"q":{"_id":&key},"u":{"_id":key,"document_schema_version":1_i64,
        "sandbox_id":state.sandbox_id.as_str(),"day":day as i64,
        "expires_at":expires(day+31*86_400),"record":encode(&record)?},
        "upsert":true,"multi":false},
    )
}
