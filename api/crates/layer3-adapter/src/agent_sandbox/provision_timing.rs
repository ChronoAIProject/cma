//! Provision timings contain only fixed stage labels and elapsed durations.
use std::time::Instant;

pub(crate) struct ProvisionTiming(Instant);

impl ProvisionTiming {
    pub(crate) fn new() -> Self {
        Self(Instant::now())
    }

    pub(crate) fn completed(&mut self, phase: &'static str) {
        let now = Instant::now();
        tracing::info!(
            phase,
            elapsed_ms = now.duration_since(self.0).as_millis(),
            "Native provision phase completed"
        );
        self.0 = now;
    }
}
