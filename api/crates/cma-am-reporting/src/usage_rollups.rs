//! Retained daily snapshots of allocated ledger projections.
use crate::store::usage_reports::{decode, encode, expires, unavailable};
use layer2_core::{
    sandbox_metrics::{allocation, MetricsError, Range, Result},
    sandbox_usage::SandboxUsage,
};
use mongodb::{
    bson::{doc, Document},
    ClientSession, Collection,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, time::Duration};

pub const COLLECTION: &str = "sandbox_usage_rollups";
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

pub async fn read_rollups(
    collection: Collection<Document>,
    session: &mut ClientSession,
    states: &[SandboxUsage],
    ids: &[&str],
    range: Range,
    deadline: Duration,
) -> Result<BTreeMap<String, allocation::Projection>> {
    if states.len() > 200 || range.seconds() > 86_400 {
        return Err(MetricsError::Busy);
    }
    let day = range.from / 86_400 * 86_400;
    let mut cached = BTreeMap::new();
    let mut cursor = collection
        .find(doc! {"sandbox_id":{"$in":ids},"day":day as i64})
        .max_time(deadline)
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
    Ok(cached)
}

pub fn match_cached_rollup<'a>(
    document: &Document,
    states: &'a [SandboxUsage],
    slices: &[Range],
) -> Result<Option<(Range, &'a SandboxUsage, allocation::Projection)>> {
    let rollup: Rollup = decode(document)?;
    let Some(slice) = slices.iter().copied().find(|slice| *slice == rollup.range) else {
        return Ok(None);
    };
    let state = states
        .iter()
        .find(|state| valid_rollup(state, slice, slice.from / 86_400 * 86_400, &rollup));
    Ok(state.map(|state| (slice, state, rollup.projection)))
}

fn valid_rollup(state: &SandboxUsage, range: Range, day: u64, rollup: &Rollup) -> bool {
    rollup.schema_version == 1
        && (rollup.projection.period.projected_tail_seconds == 0
            || state.current_allocation() == state.allocation)
        && rollup.day == day
        && rollup.range == range
        && state.sandbox_id.as_str() == rollup.sandbox_id
        && state.owner.as_str() == rollup.owner
        && (state.revision == rollup.source_watermark
            || (state.revision > rollup.source_watermark
                && rollup.projection.period.complete
                && rollup.projection.period.projected_tail_seconds == 0))
}

pub fn cache_update(
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
    let document = doc! {"_id":&key,"document_schema_version":1_i64,
    "sandbox_id":state.sandbox_id.as_str(),"day":day as i64,
    "expires_at":expires(day+31*86_400),"record":encode(&record)?};
    Ok(doc! {"q":{"_id":key},"u":[{"$replaceWith":{"$cond":[
        {"$or":[
            {"$eq":[{"$type":"$record"},"missing"]},
            {"$lt":["$record.source_watermark",state.revision as i64]},
        ]},
        {"$literal":document},"$$ROOT",
    ]}}],"upsert":true,"multi":false})
}
