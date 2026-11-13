//! Fixed-cardinality outcomes; never retain an Agent, command or output value.
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

#[derive(Clone, Copy)]
pub(super) enum Outcome {
    Bounded,
    Untrusted,
    Budget,
    Model,
    Failed,
}

static COUNTS: [AtomicU64; 5] = [const { AtomicU64::new(0) }; 5];
static CHANGED: AtomicBool = AtomicBool::new(false);

/// Errors, timeouts and cancellation retain their first failed observation.
pub(super) struct Attempt(Outcome);

impl Attempt {
    pub(super) fn new() -> Self {
        Self(Outcome::Failed)
    }

    pub(super) fn finish(&mut self, outcome: Outcome) {
        self.0 = outcome;
    }
}

impl Drop for Attempt {
    fn drop(&mut self) {
        let _ =
            COUNTS[self.0 as usize].fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                Some(value.saturating_add(1))
            });
        CHANGED.store(true, Ordering::Release);
    }
}

pub(crate) fn emit() {
    if !CHANGED.swap(false, Ordering::Acquire) {
        return;
    }
    for (outcome, counter) in [
        "bounded",
        "legacy_untrusted",
        "legacy_budget",
        "legacy_model",
        "failed",
    ]
    .into_iter()
    .zip(&COUNTS)
    {
        tracing::info!(parent: None, metric = "cma_response_source_reads_total",
            outcome, value = counter.load(Ordering::Relaxed),
            "Response source read counter");
    }
}
