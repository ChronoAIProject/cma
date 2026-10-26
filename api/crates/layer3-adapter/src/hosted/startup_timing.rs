//! Payload-free timings for the independent stages of opening a conversation.
use std::time::Instant;

pub(super) struct StartupTiming {
    last: Instant,
}

impl StartupTiming {
    pub(super) fn new() -> Self {
        Self {
            last: Instant::now(),
        }
    }

    pub(super) fn completed(&mut self, phase: &'static str) {
        let now = Instant::now();
        tracing::info!(
            phase,
            elapsed_ms = now.duration_since(self.last).as_millis(),
            "Conversation startup phase completed"
        );
        self.last = now;
    }
}
