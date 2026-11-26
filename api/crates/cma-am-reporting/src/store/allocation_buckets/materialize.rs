use super::*;
use layer2_core::sandbox_usage::UsageEvent;

/// Preserve event fold order, including floating point rounding and gap checks.
pub fn build_covered(
    state: &SandboxUsage,
    events: &[UsageEvent],
    range: Range,
    queried_to: u64,
) -> Result<DayBucket> {
    if events.len() > 100_000
        || range.from >= range.to
        || range.seconds() > DAY
        || !range.from.is_multiple_of(DAY)
        || !range.to.is_multiple_of(HOUR)
        || queried_to < range.to
        || queried_to > range.from + DAY
        || events
            .windows(2)
            .any(|pair| pair[0].revision >= pair[1].revision)
        || events.iter().any(|event| {
            event.revision > state.revision
                || event.ended_at > state.last_observed_at
                || event.started_at > event.ended_at
        })
    {
        return Err(MetricsError::Unavailable);
    }
    let foreign_scope = events
        .iter()
        .any(|e| e.sandbox_id != state.sandbox_id || e.owner != state.owner);
    let projection = |range: Range| {
        // The raw query excludes nonintersecting events before checking owner.
        // Preserve that distinction for corrupt foreign-scope input as well.
        let selected: Vec<_> = if foreign_scope {
            events
                .iter()
                .filter(|e| e.ended_at > range.from && e.started_at < range.to)
                .cloned()
                .collect()
        } else {
            Vec::new()
        };
        Slice {
            range,
            projection: allocation::project(
                state,
                if foreign_scope { &selected } else { events },
                range,
            ),
        }
    };
    let bucket = DayBucket {
        version: 1,
        sandbox_id: state.sandbox_id.to_string(),
        owner: state.owner.to_string(),
        revision: state.revision,
        source_seq: state.source_seq,
        source_revision: state.source_revision,
        allocation: state.allocation.clone(),
        created_at: state.created_at,
        tracking_started_at: state.tracking_started_at,
        complete_lifecycle: state.complete_lifecycle,
        last_observed_at: state.last_observed_at,
        day: range.from,
        total: projection(range),
        hours: (range.from..range.to)
            .step_by(HOUR as usize)
            .map(|from| {
                projection(Range {
                    from,
                    to: from + HOUR,
                })
            })
            .collect(),
        boundary: Some(boundary::Boundary::new(state, events, queried_to)),
    };
    bucket.validate()?;
    Ok(bucket)
}
