//! Aggregate observations only: no Agent, owner, content or error identifiers.
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

#[derive(Clone, Copy)]
pub(in crate::store) enum Event {
    Prepared,
    Reused,
    Failed,
    Busy,
    Fallback,
    Quarantined,
    WriteScanAvoided,
    WindowScanAvoided,
}
const NAMES: [&str; 8] = [
    "prepared",
    "reused",
    "preparation_failed",
    "preparation_busy",
    "fallback",
    "quarantined",
    "write_scan_avoided",
    "window_scan_avoided",
];
static VALUES: [AtomicU64; 8] = [const { AtomicU64::new(0) }; 8];
static CHANGED: AtomicBool = AtomicBool::new(false);

pub(in crate::store) fn record(event: Event) {
    let _ = VALUES[event as usize].fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
        Some(value.saturating_add(1))
    });
    CHANGED.store(true, Ordering::Release);
}

pub(crate) fn emit() {
    if CHANGED.swap(false, Ordering::Acquire) {
        for (counter, outcome) in VALUES.iter().zip(NAMES) {
            tracing::info!(parent: None, metric = "cma_agent_log_activation_total", outcome,
                value = counter.load(Ordering::Relaxed), "Agent log activation counter");
        }
    }
}
