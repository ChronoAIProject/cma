//! Timings expose fixed phases and durations without request or credential data.
use std::time::Instant;

pub(super) struct ProvisionTiming(Instant);

impl ProvisionTiming {
    pub(super) fn new() -> Self {
        Self(Instant::now())
    }

    pub(super) fn completed(&mut self, phase: &'static str) {
        let now = Instant::now();
        tracing::info!(
            phase,
            elapsed_ms = now.duration_since(self.0).as_millis(),
            "Sandbox provision phase completed"
        );
        self.0 = now;
    }
}
