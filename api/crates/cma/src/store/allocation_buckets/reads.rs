//! Exact daily totals and bounded prepared inputs for partial UTC days.
use super::*;
use crate::store::sandbox_usage_rollups::AllocationDelta;
use crate::store::usage_reports::{allocation_operation, ALLOCATION_DB_DEADLINE};
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
        let _timer = self.time_read("allocation_rollups")?;
        let started = std::time::Instant::now();
        let cached = allocation_operation(cma_am_reporting::allocation_buckets::read_bucket_page(
            self.collection(COLLECTION),
            session,
            states,
            range,
            ALLOCATION_DB_DEADLINE,
        ))
        .await?;
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
            let mut changed = Vec::new();
            let mut deltas = Vec::new();
            for state in states {
                let id = state.sandbox_id.to_string();
                let cached_projection = match cached.get(&(id.clone(), day)) {
                    Some(bucket) if bucket.validate_source(state)? => {
                        if bucket.total().range == selected {
                            Some(bucket.total().projection.clone())
                        } else {
                            bucket.project_boundary(state, selected)?
                        }
                    }
                    _ => None,
                };
                if let Some(projection) = cached_projection {
                    values.insert(id, projection);
                } else if let Some(revision) = cached
                    .get(&(id, day))
                    .map(|bucket| bucket.delta_revision(state, selected))
                    .transpose()?
                    .flatten()
                {
                    changed.push(state);
                    deltas.push(AllocationDelta {
                        sandbox_id: state.sandbox_id.as_str(),
                        after_revision: revision,
                        through_revision: state.revision,
                    });
                } else {
                    missing.push(state.clone());
                }
            }
            let cached_hits = values.len();
            let mut delta_hits = 0;
            if !deltas.is_empty() {
                let mut appended = self
                    .allocation_events_after(session, &deltas, selected)
                    .await?;
                for state in changed {
                    let id = state.sandbox_id.to_string();
                    let bucket = cached
                        .get(&(id.clone(), day))
                        .ok_or(MetricsError::Unavailable)?;
                    match bucket.project_delta(
                        state,
                        &appended.remove(&id).unwrap_or_default(),
                        selected,
                    )? {
                        Some(projection) => {
                            values.insert(id, projection);
                            delta_hits += 1;
                        }
                        None => missing.push(state.clone()),
                    }
                }
                metrics::counter!("cma_usage_allocation_bucket_reads_total", "outcome" => "delta")
                    .increment(delta_hits);
            }
            metrics::counter!("cma_usage_allocation_bucket_reads_total", "outcome" => "hit")
                .increment(cached_hits as u64);
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
}
