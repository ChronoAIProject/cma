//! Conservation checks for measured lifecycle evidence. This does not turn a
//! model driver into a real storage driver or qualify physical exclusion.
use super::{CycleWork, Metrics};
use crate::ledger_collector::types::{Refusal, Result, SourceFormat};

pub(super) struct Qualification {
    baseline: Metrics,
    previous: Metrics,
}

impl Qualification {
    pub(super) fn new(baseline: Metrics) -> Result<Self> {
        if !bounded(&baseline) {
            return Err(Refusal::InvalidEvidence);
        }
        Ok(Self {
            previous: baseline.clone(),
            baseline,
        })
    }

    pub(super) fn record(&mut self, work: &CycleWork, after: Metrics) -> Result<()> {
        let before = &self.previous;
        if !(1..=64).contains(&work.groups)
            || work.operations == 0
            || work.authority == 0
            || !bounded(&after)
            || after.format != self.baseline.format
            || after.active_operations != self.baseline.active_operations
            || after.active_authority != self.baseline.active_authority
            || after.preserved_source_sha256 != self.baseline.preserved_source_sha256
            || after.hot_replay_entries != self.baseline.hot_replay_entries
            || after.byte_ceiling != self.baseline.byte_ceiling
            || after.page_geometry.map(|(_, ceiling)| ceiling)
                != self.baseline.page_geometry.map(|(_, ceiling)| ceiling)
            || before.retained_groups.checked_add(work.groups) != Some(after.retained_groups)
            || before.reclaimed_operations.checked_add(work.operations)
                != Some(after.reclaimed_operations)
            || before.reclaimed_authority.checked_add(work.authority)
                != Some(after.reclaimed_authority)
        {
            return Err(Refusal::InvalidEvidence);
        }
        self.previous = after;
        Ok(())
    }

    pub(super) fn require_legacy_bounds(&self) -> Result<()> {
        if self
            .previous
            .reclaimed_operations
            .checked_sub(self.baseline.reclaimed_operations)
            .is_none_or(|count| count <= 1_024)
            || self
                .previous
                .reclaimed_authority
                .checked_sub(self.baseline.reclaimed_authority)
                .is_none_or(|count| count <= 4_096)
        {
            return Err(Refusal::InvalidEvidence);
        }
        Ok(())
    }

    pub(super) fn require_unchanged(&self, after: &Metrics) -> Result<()> {
        if *after != self.previous {
            return Err(Refusal::InvalidEvidence);
        }
        Ok(())
    }
}

fn bounded(metrics: &Metrics) -> bool {
    metrics.byte_ceiling > 0
        && metrics.active_bytes <= metrics.byte_ceiling
        && metrics.hot_progress_slots <= 1
        && metrics.preserved_source_sha256 != [0; 32]
        && match (metrics.format, metrics.page_geometry) {
            (SourceFormat::V1, None) => true,
            (SourceFormat::V2, Some((pages, ceiling))) => pages > 0 && pages <= ceiling,
            _ => false,
        }
}

#[cfg(test)]
#[path = "lifecycle_qualification_tests.rs"]
mod tests;
