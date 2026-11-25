//! Request-local reuse of a completely cold legacy allocation range.
use super::*;
use layer2_core::sandbox_usage::UsageEvent;
use std::{collections::BTreeSet, time::Instant};

mod telemetry;
use telemetry::Phases;
#[cfg(test)]
mod tests;

type Prepared = BTreeMap<u64, BTreeMap<String, allocation::Projection>>;
const DAY: u64 = 86_400;
const MAX_EXPECTATIONS: usize = 100_000;
const MAX_PREPARED_BYTES: usize = 64 * 1024 * 1024;

impl ReportingStore {
    pub(crate) async fn cold_allocation_range(
        &self,
        session: &mut mongodb::ClientSession,
        states: &[SandboxUsage],
        range: Range,
    ) -> Result<Option<Prepared>> {
        if states.len() > 200 || range.to > i64::MAX as u64 || range.seconds() > 30 * DAY {
            return Err(MetricsError::Busy);
        }
        // A one-day report already has the smallest useful raw interval.
        if states.is_empty() || range.from >= range.to || range.seconds() <= DAY {
            return Ok(None);
        }
        let _timer = self.time_read("allocation_cold_range")?;
        let mut phases = Phases::new();
        let slices = slices(range);
        let ids: Vec<_> = states.iter().map(|s| s.sandbox_id.as_str()).collect();
        let days: Vec<_> = slices.iter().map(|r| (r.from / DAY * DAY) as i64).collect();
        let began = Instant::now();
        let mut cursor = allocation_operation(async {
            self.collection(COLLECTION)
                .find(doc! {"sandbox_id":{"$in":&ids},"day":{"$in":&days}})
                .max_time(ALLOCATION_DB_DEADLINE)
                .session(&mut *session)
                .await
                .map_err(unavailable)
        })
        .await?;
        let cached_result = allocation_operation(async {
            let mut prepared = Prepared::new();
            let mut cached = BTreeSet::new();
            let mut budget = OutputBudget::default();
            loop {
                if !cursor.advance(&mut *session).await.map_err(unavailable)? {
                    break;
                }
                let document = cursor.deserialize_current().map_err(unavailable)?;
                let Some((slice, state, projection)) =
                    match_cached_rollup(&document, states, &slices)?
                else {
                    continue;
                };
                if !budget.admit(state, slice, &projection)? {
                    phases.outcome = "output_cap";
                    return Ok(None);
                }
                prepared
                    .entry(slice.from)
                    .or_default()
                    .insert(state.sandbox_id.to_string(), projection);
                cached.insert((slice.from, state.sandbox_id.to_string()));
            }
            Ok(Some((prepared, cached, budget)))
        })
        .await?;
        phases.cache_precheck = began.elapsed();
        let Some((mut prepared, cached, mut budget)) = cached_result else {
            return Ok(None);
        };

        let missing = |slice: Range, state: &SandboxUsage| {
            !cached.contains(&(slice.from, state.sandbox_id.to_string()))
        };
        for group in states.chunks(8) {
            let group_slices: Vec<_> = slices
                .iter()
                .copied()
                .filter(|slice| group.iter().any(|state| missing(*slice, state)))
                .collect();
            if group_slices.is_empty() {
                continue;
            }
            let group_states: Vec<_> = group
                .iter()
                .filter(|state| group_slices.iter().any(|slice| missing(*slice, state)))
                .cloned()
                .collect();
            let ids: Vec<_> = group_states.iter().map(|s| s.sandbox_id.as_str()).collect();
            let began = Instant::now();
            let events = self.allocation_range_events(session, &ids, range).await?;
            phases.raw += began.elapsed();
            phases.groups += 1;
            let Some(events) = events else {
                phases.outcome = "raw_cap";
                return Ok(None);
            };
            phases.events += events.values().map(Vec::len).sum::<usize>();
            let began = Instant::now();
            if !prepare_group_missing(
                &mut prepared,
                &mut budget,
                &group_states,
                &events,
                &group_slices,
                &cached,
            )? {
                phases.outcome = "output_cap";
                return Ok(None);
            }
            phases.projection += began.elapsed();
            // All raw histories for this group are released before the next.
        }
        // No cache writes or caller-visible map precede full validation/caps.
        let began = Instant::now();
        for slice in &slices {
            let _timer = self.time_read("allocation_cold_range_cache_write")?;
            let Some(projections) = prepared.get(&slice.from) else {
                return Err(MetricsError::Unavailable);
            };
            let updates = states
                .iter()
                .filter(|state| missing(*slice, state))
                .map(|state| {
                    let projection = projections
                        .get(state.sandbox_id.as_str())
                        .ok_or(MetricsError::Unavailable)?;
                    cache_update(state, *slice, projection)
                })
                .collect::<Result<Vec<_>>>()?;
            if !updates.is_empty() {
                allocation_operation(self.allocation_cache_updates(updates)).await?;
            }
        }
        phases.cache_write = began.elapsed();
        phases.outcome = if cached.is_empty() {
            "prepared"
        } else if cached.len() == states.len() * slices.len() {
            "cache_reused"
        } else {
            "cache_repaired"
        };
        Ok(Some(prepared))
    }
}

fn slices(range: Range) -> Vec<Range> {
    let mut slices = Vec::new();
    let mut from = range.from;
    while from < range.to {
        let to = ((from / DAY + 1) * DAY).min(range.to);
        slices.push(Range { from, to });
        from = to;
    }
    slices
}

#[derive(Default)]
struct OutputBudget {
    expectations: usize,
    bytes: usize,
}

impl OutputBudget {
    fn admit(
        &mut self,
        state: &SandboxUsage,
        range: Range,
        projection: &allocation::Projection,
    ) -> Result<bool> {
        // Bound both variable-sized expectations and serialized cache output.
        // At most 31 * 200 projections/identity strings can be retained.
        self.expectations = self
            .expectations
            .saturating_add(projection.expectations.len());
        if self.expectations > MAX_EXPECTATIONS {
            return Ok(false);
        }
        let update = cache_update(state, range, projection)?;
        let bytes = mongodb::bson::serialize_to_vec(&update)
            .map_err(unavailable)?
            .len();
        self.bytes = self.bytes.saturating_add(bytes);
        Ok(self.bytes <= MAX_PREPARED_BYTES)
    }
}

fn prepare_group_missing(
    prepared: &mut Prepared,
    budget: &mut OutputBudget,
    states: &[SandboxUsage],
    events: &BTreeMap<String, Vec<UsageEvent>>,
    slices: &[Range],
    cached: &BTreeSet<(u64, String)>,
) -> Result<bool> {
    for state in states {
        let events = events
            .get(state.sandbox_id.as_str())
            .map(Vec::as_slice)
            .unwrap_or_default();
        for slice in slices {
            if cached.contains(&(slice.from, state.sandbox_id.to_string())) {
                continue;
            }
            // Filtering must precede project: foreign-owner events outside this
            // day must not change its completeness. Crossing events stay in all
            // overlapping days, in the same full-width revision order.
            let selected: Vec<_> = events
                .iter()
                .filter(|event| event.ended_at > slice.from && event.started_at < slice.to)
                .cloned()
                .collect();
            let projection = allocation::project(state, &selected, *slice);
            if !budget.admit(state, *slice, &projection)? {
                return Ok(false);
            }
            prepared
                .entry(slice.from)
                .or_default()
                .insert(state.sandbox_id.to_string(), projection);
        }
    }
    Ok(true)
}
