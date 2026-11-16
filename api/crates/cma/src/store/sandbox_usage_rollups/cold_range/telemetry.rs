//! Aggregate phase clocks only; no eager driver callback or customer identities.
use std::time::{Duration, Instant};

pub(super) struct Phases {
    started: Instant,
    pub cache_precheck: Duration,
    pub raw: Duration,
    pub projection: Duration,
    pub cache_write: Duration,
    pub groups: usize,
    pub events: usize,
    pub outcome: &'static str,
}

impl Phases {
    pub fn new() -> Self {
        Self {
            started: Instant::now(),
            cache_precheck: Duration::ZERO,
            raw: Duration::ZERO,
            projection: Duration::ZERO,
            cache_write: Duration::ZERO,
            groups: 0,
            events: 0,
            outcome: "incomplete",
        }
    }
}

impl Drop for Phases {
    fn drop(&mut self) {
        tracing::info!(
            event = "usage_allocation_cold_range",
            outcome = self.outcome,
            groups = self.groups,
            events = self.events,
            cache_precheck_us = self.cache_precheck.as_micros() as u64,
            raw_validation_us = self.raw.as_micros() as u64,
            projection_us = self.projection.as_micros() as u64,
            cache_write_us = self.cache_write.as_micros() as u64,
            elapsed_us = self.started.elapsed().as_micros() as u64,
            "Cold allocation range preparation"
        );
    }
}
