pub(super) use cma_am_reporting::allocation_buckets::build_covered;

#[cfg(test)]
pub(super) fn build(
    state: &super::SandboxUsage,
    events: &[layer2_core::sandbox_usage::UsageEvent],
    range: super::Range,
) -> super::Result<super::DayBucket> {
    build_covered(state, events, range, range.to)
}
