//! Bounded BSON reads for complete days and partial-day projections.
use super::*;
use mongodb::{ClientSession, Collection};
use std::{collections::BTreeMap, time::Duration};

const SANDBOXES_PER_CURSOR: usize = 32;
const DAYS_PER_SANDBOX: usize = 31;

#[cfg(test)]
mod tests;

pub async fn read_bucket_page(
    collection: Collection<Document>,
    session: &mut ClientSession,
    states: &[SandboxUsage],
    range: Range,
    deadline: Duration,
) -> Result<BTreeMap<(String, u64), DayBucket>> {
    if states.len() > 200 || range.seconds() > 30 * DAY || range.from > range.to {
        return Err(MetricsError::Busy);
    }
    if states.is_empty() || range.from == range.to {
        return Ok(BTreeMap::new());
    }
    let mut ids: Vec<_> = states
        .iter()
        .map(|state| state.sandbox_id.as_str())
        .collect();
    ids.sort_unstable();
    ids.dedup();
    let mut edges = Vec::with_capacity(2);
    if !range.from.is_multiple_of(DAY) {
        edges.push((range.from / DAY * DAY) as i64);
    }
    if !range.to.is_multiple_of(DAY) {
        let last = (range.to / DAY * DAY) as i64;
        if !edges.contains(&last) {
            edges.push(last);
        }
    }
    let mut out = BTreeMap::new();
    let mut bytes = 0;
    // Fetch each document once. Only the at-most-two partial dates carry
    // original projection inputs; interior history transfers daily totals.
    for boundary in [false, true] {
        if boundary && edges.is_empty() {
            continue;
        }
        let dates = if boundary {
            doc! {"$in":&edges}
        } else {
            doc! {"$gte":(range.from / DAY * DAY) as i64,"$lt":range.to as i64,"$nin":&edges}
        };
        let mut projection = doc! {"record":0,"content_sha256":0};
        if !boundary {
            projection.extend(doc! {"boundary_record":0,"boundary_sha256":0});
        }
        // A dense month can consume one cursor's server budget over several
        // batches. Bound each cursor while using the caller's same snapshot.
        for group in ids.chunks(SANDBOXES_PER_CURSOR) {
            let mut cursor = collection
                .find(doc! {"sandbox_id":{"$in":group},"day":dates.clone()})
                .projection(projection.clone())
                .batch_size((group.len() * DAYS_PER_SANDBOX) as u32)
                .max_time(deadline)
                .session(&mut *session)
                .await
                .map_err(unavailable)?;
            while let Some(document) = cursor
                .next(&mut *session)
                .await
                .transpose()
                .map_err(unavailable)?
            {
                bytes += mongodb::bson::serialize_to_vec(&document)
                    .map_err(unavailable)?
                    .len();
                if bytes > 64 * 1024 * 1024 {
                    return Err(MetricsError::Busy);
                }
                let bucket = if boundary {
                    DayBucket::read_boundary(&document)?
                } else {
                    DayBucket::read_summary(&document)?
                };
                if !group.contains(&bucket.sandbox_id.as_str())
                    || bucket.day < range.from / DAY * DAY
                    || bucket.day >= range.to
                    || out.len() >= ids.len() * DAYS_PER_SANDBOX
                    || out
                        .insert((bucket.sandbox_id.clone(), bucket.day), bucket)
                        .is_some()
                {
                    return Err(MetricsError::Unavailable);
                }
            }
        }
    }
    Ok(out)
}
