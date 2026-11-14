//! Exact daily totals and bounded prepared inputs for partial UTC days.
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
        let started = std::time::Instant::now();
        let cached = self.allocation_bucket_page(session, states, range).await?;
        let cache_us = started.elapsed().as_micros();
        metrics::histogram!("cma_usage_allocation_bucket_read_seconds", "phase" => "cache")
            .record(started.elapsed().as_secs_f64());
        #[cfg(test)]
        eprintln!(
            "bucket_cache_read members={} seconds={} docs={} cache_us={cache_us}",
            states.len(),
            range.seconds(),
            cached.len()
        );
        let _ = cache_us;
        let mut out = BTreeMap::new();
        let mut from = range.from;
        while from < range.to {
            let day = from / DAY * DAY;
            let selected = Range {
                from,
                to: (day + DAY).min(range.to),
            };
            let mut values = BTreeMap::new();
            let mut missing = Vec::new();
            for state in states {
                let id = state.sandbox_id.to_string();
                let cached_projection = match cached.get(&(id.clone(), day)) {
                    Some(bucket) if bucket.validate_source(state)? => {
                        if bucket.total.range == selected {
                            Some(bucket.total.projection.clone())
                        } else {
                            bucket.project_boundary(state, selected)?
                        }
                    }
                    _ => None,
                };
                if let Some(projection) = cached_projection {
                    values.insert(id, projection);
                } else {
                    missing.push(state.clone());
                }
            }
            metrics::counter!("cma_usage_allocation_bucket_reads_total", "outcome" => "hit")
                .increment(values.len() as u64);
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
        if states.len() > 200 || range.seconds() > 30 * DAY || range.from > range.to {
            return Err(MetricsError::Busy);
        }
        if states.is_empty() || range.from == range.to {
            return Ok(BTreeMap::new());
        }
        let ids: Vec<_> = states
            .iter()
            .map(|state| state.sandbox_id.as_str())
            .collect();
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
            let mut cursor = self
                .collection(COLLECTION)
                .find(doc! {"sandbox_id":{"$in":&ids},"day":dates})
                .projection(projection)
                .batch_size(2000)
                .max_time(DB_DEADLINE)
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
                if !ids.contains(&bucket.sandbox_id.as_str())
                    || bucket.day < range.from / DAY * DAY
                    || bucket.day >= range.to
                    || out.len() >= states.len() * 31
                    || out
                        .insert((bucket.sandbox_id.clone(), bucket.day), bucket)
                        .is_some()
                {
                    return Err(MetricsError::Unavailable);
                }
            }
        }
        Ok(out)
    }
}
