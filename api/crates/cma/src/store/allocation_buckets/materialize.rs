use super::*;
use layer2_core::sandbox_usage::UsageEvent;

/// Preserve the original event fold order. Summing independently rounded hour
/// totals would change the daily f64 result and hide cross-hour revision gaps.
pub(super) fn build(
    state: &SandboxUsage,
    events: &[UsageEvent],
    range: Range,
) -> Result<DayBucket> {
    if events.len() > 100_000
        || range.from >= range.to
        || range.seconds() > DAY
        || !range.from.is_multiple_of(DAY)
        || !range.to.is_multiple_of(HOUR)
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
    let projection = |range| Slice {
        range,
        projection: allocation::project(state, events, range),
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
    };
    bucket.validate()?;
    Ok(bucket)
}
