//! Replayable allocation-day projection. The allocated ledger is read-only.
use super::usage_reports::{decode, encode, expires, unavailable, ReportingStore, DB_DEADLINE};
use layer2_core::{
    sandbox_metrics::{allocation, MetricsError, Range, Result},
    sandbox_usage::SandboxUsage,
};
use mongodb::bson::doc;
use serde::{Deserialize, Serialize};
mod source;
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
            .max_time(DB_DEADLINE)
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
            if r.schema_version == 1
                && r.range == range
                && states.iter().any(|s| {
                    s.sandbox_id.as_str() == r.sandbox_id
                        && s.owner.as_str() == r.owner
                        && (s.revision == r.source_watermark
                            || (s.revision > r.source_watermark
                                && r.projection.period.complete
                                && r.projection.period.projected_tail_seconds == 0))
                })
            {
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
            let r = Rollup {
                schema_version: 1,
                sandbox_id: s.sandbox_id.to_string(),
                owner: s.owner.to_string(),
                day,
                source_watermark: s.revision,
                range,
                projection: p.clone(),
            };
            let key = format!("{}:{day}", s.sandbox_id);
            updates.push(doc!{"q":{"_id":&key},"u":{"_id":key,"document_schema_version":1_i64,"sandbox_id":s.sandbox_id.as_str(),"day":day as i64,"expires_at":expires(day+31*86_400),"record":encode(&r)?},"upsert":true,"multi":false});
            cached.insert(s.sandbox_id.to_string(), p);
        }
        if !updates.is_empty() {
            self.bulk_updates(COLLECTION, updates).await?;
        }
        Ok(cached)
    }
}
