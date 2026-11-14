//! Exact cached day reads; unaligned edges retain the original event fold.
use super::*;
use std::collections::BTreeMap;

impl ReportingStore {
    pub(crate) async fn bucketed_allocation_range(
        &self,
        session: &mut mongodb::ClientSession,
        states: &[SandboxUsage],
        range: Range,
    ) -> Result<BTreeMap<u64, BTreeMap<String, allocation::Projection>>> {
        if !self.view_policy.allocation_rollups_enabled {
            return Err(MetricsError::ViewDisabled);
        }
        let cached = self.allocation_bucket_page(session, states, range).await?;
        let mut out = BTreeMap::new();
        let mut from = range.from;
        while from < range.to {
            let day = from / DAY * DAY;
            let selected = Range { from, to: (day + DAY).min(range.to) };
            let mut values = BTreeMap::new();
            let mut missing = Vec::new();
            for state in states {
                let id = state.sandbox_id.to_string();
                let cached_slice = match cached.get(&(id.clone(), day)) {
                    Some(bucket) if bucket.validate_source(state)? => {
                        if bucket.total.range == selected {
                            Some(&bucket.total)
                        } else {
                            bucket.hours.iter().find(|hour| hour.range == selected)
                        }
                    }
                    _ => None,
                };
                if let Some(slice) = cached_slice {
                    values.insert(id, slice.projection.clone());
                } else {
                    missing.push(state.clone());
                }
            }
            metrics::counter!("cma_usage_allocation_bucket_reads_total", "outcome" => "hit").increment(values.len() as u64);
            metrics::counter!("cma_usage_allocation_bucket_reads_total", "outcome" => "raw_fallback").increment(missing.len() as u64);
            if !missing.is_empty() {
                // Do not sum/subtract floating point hourly slices: that changes
                // the original event order and can hide cross-hour corruption.
                values.extend(self.allocation_periods(session, &missing, selected).await?);
            }
            out.insert(from, values);
            from = selected.to;
        }
        Ok(out)
    }

    async fn allocation_bucket_page(
        &self,
        session: &mut mongodb::ClientSession,
        states: &[SandboxUsage],
        range: Range,
    ) -> Result<BTreeMap<(String, u64), DayBucket>> {
        if states.len() > 200 || range.seconds() > 30 * DAY || range.from >= range.to {
            return Err(MetricsError::Busy);
        }
        if states.is_empty() { return Ok(BTreeMap::new()); }
        let ids: Vec<_> = states.iter().map(|state| state.sandbox_id.as_str()).collect();
        let summary_only = range.seconds() != HOUR || !range.from.is_multiple_of(HOUR);
        let mut cursor = self.collection(COLLECTION)
            .find(doc! {"sandbox_id":{"$in":&ids},"day":{"$gte":(range.from / DAY * DAY) as i64,"$lt":range.to as i64}})
            .projection(if summary_only { doc! {"record":0,"content_sha256":0} } else { doc! {} })
            .batch_size(200).max_time(DB_DEADLINE).session(&mut *session)
            .await.map_err(unavailable)?;
        let mut out = BTreeMap::new();
        let mut bytes = 0;
        while let Some(document) = cursor.next(&mut *session).await.transpose().map_err(unavailable)? {
            bytes += mongodb::bson::serialize_to_vec(&document).map_err(unavailable)?.len();
            if bytes > 64 * 1024 * 1024 { return Err(MetricsError::Busy); }
            let bucket = if summary_only { DayBucket::read_summary(&document)? } else { DayBucket::read(&document)? };
            if !ids.contains(&bucket.sandbox_id.as_str()) || bucket.day < range.from / DAY * DAY
                || bucket.day >= range.to || out.len() >= states.len() * 31
                || out.insert((bucket.sandbox_id.clone(), bucket.day), bucket).is_some() {
                return Err(MetricsError::Unavailable);
            }
        }
        Ok(out)
    }
}
