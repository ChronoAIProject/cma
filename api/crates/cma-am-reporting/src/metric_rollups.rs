//! Stored per-minute aggregates and latest readings.
use crate::store::usage_reports::{decode, encode, expires, unavailable};
use layer2_core::sandbox_metrics::{
    Aggregate, MetricRollup, MetricsError, Range, Result, ALGORITHM_VERSION, RETENTION_SECONDS,
};
use mongodb::{
    bson::{doc, Document},
    ClientSession, Collection,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, time::Duration};

pub const COLLECTION: &str = "sandbox_metric_rollups";

pub fn metric_updates(rollups: &[MetricRollup]) -> Result<Vec<Document>> {
    let mut updates = Vec::new();
    for rollup in rollups {
        if rollup.minutes.len() > 60 || rollup.algorithm_version != ALGORITHM_VERSION {
            return Err(MetricsError::Unavailable);
        }
        let key = format!("{}:{}:{}", rollup.sandbox_id, rollup.day, rollup.chunk);
        let d = doc! {"_id":&key,"document_schema_version":1_i64,"sandbox_id":&rollup.sandbox_id,"owner":&rollup.owner_subject,
        "day":rollup.day as i64,"chunk":rollup.chunk as i64,"cutoff":rollup.cutoff as i64,
        "expires_at":expires(rollup.day + (u64::from(rollup.chunk) + 1) * 3600 + RETENTION_SECONDS),"record":encode(rollup)?};
        // Upsert by stable identity, then fence in the update pipeline. A
        // partial retry never erases observations already stored by a
        // successful source read, and an older worker cannot move cutoff back.
        updates.push(doc! {"q":{"_id":key},"u":[{"$replaceWith":{"$cond":[
        {"$or":[{"$eq":[{"$type":"$record"},"missing"]},
            {"$and":[{"$lte":["$cutoff",rollup.cutoff as i64]},rollup.source_complete]}]},
        {"$literal":d},"$$ROOT"]}}],"upsert":true,"multi":false});
    }
    Ok(updates)
}

pub async fn read_metric_rollups(
    collection: Collection<Document>,
    session: &mut ClientSession,
    ids: &[String],
    range: Range,
    deadline: Duration,
) -> Result<Vec<MetricRollup>> {
    if ids.len() > 200 || range.seconds() > 86_400 {
        return Err(MetricsError::Busy);
    }
    let first_day = range.from / 86_400 * 86_400;
    let last_day = range.to.saturating_sub(1) / 86_400 * 86_400;
    let filter = doc! {
        "sandbox_id": {"$in": ids},
        "day": {"$gte": first_day as i64, "$lte": last_day as i64},
    };
    let mut cursor = collection
        .find(filter)
        .max_time(deadline)
        .session(&mut *session)
        .await
        .map_err(unavailable)?;
    let mut out = Vec::new();
    while let Some(d) = cursor
        .next(&mut *session)
        .await
        .transpose()
        .map_err(unavailable)?
    {
        let value: MetricRollup = decode(&d)?;
        if value.schema_version != 1 || value.algorithm_version != ALGORITHM_VERSION {
            return Err(MetricsError::Unavailable);
        }
        out.push(value);
        if out.len() > 4800 {
            return Err(MetricsError::Busy);
        }
    }
    Ok(out)
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveReading {
    pub owner: String,
    pub observed_at: u64,
    pub metrics: [Aggregate; 3],
}

pub fn live_updates(readings: &[(String, LiveReading)]) -> Result<Vec<Document>> {
    let mut updates = Vec::new();
    for (id, reading) in readings {
        let key = format!("{id}:live");
        let d = doc! {"_id":&key,"document_schema_version":1_i64,"sandbox_id":id,"day":-1_i64,"chunk":0_i64,
        "expires_at":expires(reading.observed_at+RETENTION_SECONDS),"record":encode(reading)?};
        updates.push(doc! {"q":{"_id":key},"u":[{"$replaceWith":{"$cond":[
                {"$or":[{"$eq":[{"$type":"$record"},"missing"]},{"$lte":["$record.observed_at",reading.observed_at as i64]}]},
                {"$literal":d},"$$ROOT"]}}],"upsert":true,"multi":false});
    }
    Ok(updates)
}

pub async fn read_live(
    collection: Collection<Document>,
    ids: &[String],
    session: &mut ClientSession,
    deadline: Duration,
) -> Result<BTreeMap<String, LiveReading>> {
    let mut cursor = collection
        .find(doc! {"sandbox_id":{"$in":ids},"day":-1_i64})
        .max_time(deadline)
        .session(&mut *session)
        .await
        .map_err(unavailable)?;
    let mut out = BTreeMap::new();
    while let Some(d) = cursor
        .next(&mut *session)
        .await
        .transpose()
        .map_err(unavailable)?
    {
        out.insert(
            d.get_str("sandbox_id").map_err(unavailable)?.into(),
            decode(&d)?,
        );
    }
    Ok(out)
}
