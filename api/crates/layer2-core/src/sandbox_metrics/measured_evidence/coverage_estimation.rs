//! Interior-gap estimates require unique, continuously allocated neighbors.
use super::{
    quantity::{exact, whole},
    AllocationSpan, Candidate, Unresolved, LATE_GRACE_MS, MAX_INTERIOR_GAP_MS,
};

pub(super) fn continuously_allocated(
    allocation: &[AllocationSpan<'_>],
    candidate: &Candidate,
) -> bool {
    let (from, to) = (candidate.interval.from_ms, candidate.interval.to_ms);
    let mut cursor = from;
    for span in allocation
        .iter()
        .filter(|span| span.from_ms < to && from < span.to_ms)
    {
        let allocation = &span.event.allocation;
        let eligible = if candidate.interval.metric.is_pvc() {
            allocation.disk_gib > 0
        } else {
            allocation.running
        };
        if span.from_ms > cursor || !eligible {
            return false;
        }
        cursor = cursor.max(span.to_ms);
    }
    cursor >= to
}

fn uniquely_attributed(
    candidate: &Candidate,
    candidates: &[Candidate],
    allocation: &[AllocationSpan<'_>],
) -> bool {
    candidate.attributed
        && continuously_allocated(allocation, candidate)
        && !candidates.iter().any(|other| {
            other.series != candidate.series
                && other.interval.metric == candidate.interval.metric
                && other.interval.from_ms < candidate.interval.to_ms
                && candidate.interval.from_ms < other.interval.to_ms
        })
}

pub(super) fn estimate_interior(
    candidates: &mut [Candidate],
    cutoff_ms: u64,
    allocation: &[AllocationSpan<'_>],
) {
    for index in 1..candidates.len().saturating_sub(1) {
        let (left, gap, right) = (
            &candidates[index - 1],
            &candidates[index],
            &candidates[index + 1],
        );
        let elapsed = gap.interval.to_ms - gap.interval.from_ms;
        if !gap.attributed
            || gap.interval.quantity != Err(Unresolved::Coverage)
            || elapsed > MAX_INTERIOR_GAP_MS
            || cutoff_ms.saturating_sub(gap.interval.to_ms) < LATE_GRACE_MS
            || left.series != gap.series
            || right.series != gap.series
            || !uniquely_attributed(left, candidates, allocation)
            || !uniquely_attributed(right, candidates, allocation)
            || left.binding != gap.binding
            || right.binding != gap.binding
            || left.interval.to_ms != gap.interval.from_ms
            || gap.interval.to_ms != right.interval.from_ms
        {
            continue;
        }
        let (Ok(a), Ok(b)) = (left.interval.quantity, right.interval.quantity) else {
            continue;
        };
        let left_rate = exact(a) / whole(left.interval.to_ms - left.interval.from_ms);
        let right_rate = exact(b) / whole(right.interval.to_ms - right.interval.from_ms);
        let estimate = (left_rate + right_rate) * whole(elapsed) / whole(2_u64);
        let mut references = Vec::new();
        for reference in left
            .references
            .iter()
            .chain(&gap.references)
            .chain(&right.references)
        {
            if references.last() != Some(reference) {
                references.push(reference.clone());
            }
        }
        candidates[index].estimate = Some(estimate);
        candidates[index].references = references;
    }
}
