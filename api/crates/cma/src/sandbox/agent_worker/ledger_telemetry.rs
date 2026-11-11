//! Process-local failure observations on the API's structured metrics log path.
use std::sync::atomic::{AtomicU64, Ordering};

use super::support::WorkerFailure;

static HOLDS: AtomicU64 = AtomicU64::new(0);

pub(super) fn observe(failure: &WorkerFailure) {
    record(&HOLDS, failure);
}

fn record(counter: &AtomicU64, failure: &WorkerFailure) {
    if !failure.is_ledger_hold() {
        return;
    }
    let previous = counter
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
            Some(count.saturating_add(1))
        })
        .unwrap_or_else(|count| count);
    // No runtime observation endpoint is called. In particular, an old runtime
    // reports neither occupancy nor a safe free-slot estimate. Never log detail
    // or inherit caller spans containing Agent/Sandbox identifiers or content.
    tracing::warn!(parent: None,
        reason = "runtime_ledger_capacity",
        failures_total = previous.saturating_add(1),
        occupancy = "unknown", occupancy_support = "unknown",
        "agent runtime ledger hold observed"
    );
}

#[cfg(test)]
mod tests;
