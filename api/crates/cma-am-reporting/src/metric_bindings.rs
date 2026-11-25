//! Bounded reads of retained physical assignment intervals.
use crate::store::usage_reports::{decode, unavailable};
use futures_util::TryStreamExt;
use layer2_core::sandbox_metrics::{Binding, MetricsError, Range, Result};
use mongodb::{
    bson::{doc, Document},
    ClientSession, Collection,
};
use std::time::Duration;

pub const COLLECTION: &str = "sandbox_metric_bindings";

pub async fn read_sandbox_page(
    collection: Collection<Document>,
    after: Option<&str>,
    range: Range,
    deadline: Duration,
) -> Result<Vec<String>> {
    let mut filter = doc! {"valid_from":{"$lt":range.to as i64},"$or":[{"valid_until":null},{"valid_until":{"$gt":range.from as i64}}]};
    if let Some(after) = after {
        filter.insert("sandbox_id", doc! {"$gt":after});
    }
    let mut cursor = collection
        .find(filter)
        .sort(doc! {"sandbox_id":1,"valid_from":1})
        .projection(doc! {"sandbox_id":1})
        .limit(200)
        .batch_size(200)
        .max_time(deadline)
        .await
        .map_err(unavailable)?;
    let mut ids = Vec::new();
    while let Some(d) = cursor.try_next().await.map_err(unavailable)? {
        let id = d.get_str("sandbox_id").map_err(unavailable)?;
        if ids.last().is_none_or(|last| last != id) {
            ids.push(id.to_owned());
        }
        if ids.len() == 100 {
            break;
        }
    }
    Ok(ids)
}

pub async fn read_bindings(
    collection: Collection<Document>,
    ids: &[String],
    range: Range,
    session: &mut ClientSession,
    deadline: Duration,
) -> Result<Vec<Binding>> {
    if ids.len() > 200 {
        return Err(MetricsError::Busy);
    }
    let filter = doc! {
        "sandbox_id": {"$in": ids},
        "valid_from": {"$lt": range.to as i64},
        "$or": [{"valid_until": null}, {"valid_until": {"$gt": range.from as i64}}],
    };
    let mut cursor = collection
        .find(filter)
        .sort(doc! {"sandbox_id": 1, "valid_from": 1})
        .max_time(deadline)
        .session(&mut *session)
        .await
        .map_err(unavailable)?;
    let mut rows = Vec::new();
    while let Some(doc) = cursor
        .next(&mut *session)
        .await
        .transpose()
        .map_err(unavailable)?
    {
        if rows.len() >= 20_000 {
            return Err(MetricsError::Busy);
        }
        rows.push(decode(&doc)?);
    }
    Ok(rows)
}

pub async fn read_tracking_start(
    collection: Collection<Document>,
    ids: &[String],
    session: &mut ClientSession,
    deadline: Duration,
) -> Result<Option<u64>> {
    let mut cursor = collection
        .aggregate([
            doc! {"$match":{"sandbox_id":{"$in":ids}}},
            doc! {"$group":{"_id":null,"from":{"$min":"$valid_from"}}},
        ])
        .max_time(deadline)
        .session(&mut *session)
        .await
        .map_err(unavailable)?;
    let document = cursor
        .next(session)
        .await
        .transpose()
        .map_err(unavailable)?;
    document
        .map(|d| d.get_i64("from").map(|v| v as u64).map_err(unavailable))
        .transpose()
}
